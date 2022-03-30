/*!
  ec2 provider helps launching bottlerocket nodes and connect to EKS cluster.
  Meanwhile, terminating all created ec2 instances when integration test is running 'clean' subcommand.
!*/

use aws_config::meta::region::RegionProviderChain;
use aws_sdk_ec2::model::{
    ArchitectureValues, Filter, IamInstanceProfileSpecification, InstanceType, ResourceType, Tag,
    TagSpecification,
};
use aws_sdk_ec2::Region;

use crate::eks_provider::ClusterInfo;
use crate::error::{IntoProviderError, ProviderError, ProviderResult};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Debug;
use std::iter::FromIterator;
use std::time::Duration;

/// The default number of instances to spin up.
const DEFAULT_INSTANCE_COUNT: i32 = 3;
/// The tag name used to create instances.
const INSTANCE_TAG_NAME: &str = "brupop";
const INSTANCE_TAG_VALUE: &str = "integration-test";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct CreatedEc2Instances {
    /// The ids of all created instances
    pub instance_ids: HashSet<String>,

    /// The private dns name (node name) of all created instances
    pub private_dns_name: Vec<String>,
}

pub struct Ec2Creator {}

impl Ec2Creator {}

pub async fn create_ec2_instance(
    cluster: ClusterInfo,
    ami_arch: &str,
    bottlerocket_version: &str,
) -> ProviderResult<CreatedEc2Instances> {
    // Setup aws_sdk_config and clients.
    let region_provider = RegionProviderChain::first_try(Some(Region::new(cluster.region.clone())));
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let ec2_client = aws_sdk_ec2::Client::new(&shared_config);
    let ssm_client = aws_sdk_ssm::Client::new(&shared_config);

    // Prepare security groups
    let mut security_groups = vec![];
    security_groups.append(&mut cluster.nodegroup_sg.clone());
    security_groups.append(&mut cluster.clustershared_sg.clone());

    // Prepare ami id
    //default eks_version to the version that matches cluster
    let eks_version = cluster.version;
    let node_ami = find_ami_id(&ssm_client, ami_arch, bottlerocket_version, &eks_version).await?;

    // Prepare instance type
    let instance_type = instance_type(&ec2_client, &node_ami).await?;

    // Run the ec2 instances
    let run_instances = ec2_client
        .run_instances()
        .min_count(DEFAULT_INSTANCE_COUNT)
        .max_count(DEFAULT_INSTANCE_COUNT)
        .subnet_id(first_subnet_id(&cluster.private_subnet_ids)?)
        .set_security_group_ids(Some(security_groups))
        .image_id(node_ami)
        .instance_type(InstanceType::from(instance_type.as_str()))
        .tag_specifications(tag_specifications(&cluster.name))
        .user_data(userdata(
            &cluster.endpoint,
            &cluster.name,
            &cluster.certificate,
        ))
        .iam_instance_profile(
            IamInstanceProfileSpecification::builder()
                .arn(&cluster.iam_instance_profile_arn)
                .build(),
        );

    let instances = run_instances
        .send()
        .await
        .context("Failed to create instances")?
        .instances
        .context("Results missing instances field")?;
    let mut instance_ids = HashSet::new();
    let mut private_dns_name: Vec<String> = Vec::new();
    for instance in instances {
        instance_ids.insert(instance.instance_id.clone().ok_or_else(|| {
            ProviderError::new_with_context("Instance missing instance_id field")
        })?);
        private_dns_name.push(instance.private_dns_name.clone().ok_or_else(|| {
            ProviderError::new_with_context("Instance missing private_dns_name field")
        })?);
    }

    // Ensure the instances reach a running state.
    tokio::time::timeout(
        Duration::from_secs(60),
        wait_for_conforming_instances(&ec2_client, &instance_ids, DesiredInstanceState::Running),
    )
    .await
    .context("Timed-out waiting for instances to reach the `running` state.")??;

    // Return the ids for the created instances.
    Ok(CreatedEc2Instances {
        instance_ids: instance_ids,
        private_dns_name: private_dns_name,
    })
}

pub async fn terminate_ec2_instance(cluster: ClusterInfo) -> ProviderResult<()> {
    // Setup aws_sdk_config and clients.
    let region_provider = RegionProviderChain::first_try(Some(Region::new(cluster.region.clone())));
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let ec2_client = aws_sdk_ec2::Client::new(&shared_config);

    let running_instance_ids = get_instances_by_tag(&ec2_client).await?;

    let _terminate_results = ec2_client
        .terminate_instances()
        .set_instance_ids(Some(Vec::from_iter(running_instance_ids.clone())))
        .send()
        .await
        .map_err(|e| {
            ProviderError::new_with_source_and_context("Failed to terminate instances", e)
        })?;
    // Ensure the instances reach a terminated state.
    tokio::time::timeout(
        Duration::from_secs(300),
        wait_for_conforming_instances(
            &ec2_client,
            &running_instance_ids,
            DesiredInstanceState::Terminated,
        ),
    )
    .await
    .context("Timed-out waiting for instances to reach the `terminated` state.")??;
    Ok(())
}

// Find the node ami id to use.
async fn find_ami_id(
    ssm_client: &aws_sdk_ssm::Client,
    arch: &str,
    br_version: &str,
    eks_version: &str,
) -> ProviderResult<String> {
    let parameter_name = format!(
        "/aws/service/bottlerocket/aws-k8s-{}/{}/{}/image_id",
        eks_version, arch, br_version
    );
    let ami_id = ssm_client
        .get_parameter()
        .name(parameter_name)
        .send()
        .await
        .context("Unable to get ami id")?
        .parameter
        .context("Unable to get ami id")?
        .value
        .context("ami id is missing")?;
    Ok(ami_id)
}

/// Determine the instance type to use. If provided use that one. Otherwise, for `x86_64` use `m5.large`
/// and for `aarch64` use `m6g.large`
async fn instance_type(ec2_client: &aws_sdk_ec2::Client, node_ami: &str) -> ProviderResult<String> {
    let arch = ec2_client
        .describe_images()
        .image_ids(node_ami)
        .send()
        .await
        .context("Unable to get ami architecture")?
        .images
        .context("Unable to get ami architecture")?
        .get(0)
        .context("Unable to get ami architecture")?
        .architecture
        .clone()
        .context("Ami has no architecture")?;

    Ok(match arch {
        ArchitectureValues::X8664 => "m5.large",
        ArchitectureValues::Arm64 => "m6g.large",
        _ => "m6g.large",
    }
    .to_string())
}

fn first_subnet_id(subnet_ids: &[String]) -> ProviderResult<String> {
    subnet_ids
        .get(0)
        .map(|id| id.to_string())
        .context("There are no private subnet ids")
}

fn tag_specifications(cluster_name: &str) -> TagSpecification {
    TagSpecification::builder()
        .resource_type(ResourceType::Instance)
        .tags(
            Tag::builder()
                .key("Name")
                .value(format!("{}_node", cluster_name))
                .build(),
        )
        .tags(
            Tag::builder()
                .key(format!("kubernetes.io/cluster/{}", cluster_name))
                .value("owned")
                .build(),
        )
        .tags(
            Tag::builder()
                .key(INSTANCE_TAG_NAME)
                .value(INSTANCE_TAG_VALUE)
                .build(),
        )
        .build()
}

fn userdata(endpoint: &str, cluster_name: &str, certificate: &str) -> String {
    base64::encode(format!(
        r#"[settings.updates]
ignore-waves = true
    
[settings.kubernetes]
api-server = "{}"
cluster-name = "{}"
cluster-certificate = "{}""#,
        endpoint, cluster_name, certificate
    ))
}

#[derive(Debug)]
enum DesiredInstanceState {
    Running,
    Terminated,
}

impl DesiredInstanceState {
    fn filter(&self) -> Filter {
        let filter = Filter::builder()
            .name("instance-state-name")
            .values("pending")
            .values("shutting-down")
            .values("stopping")
            .values("stopped")
            .values(match self {
                DesiredInstanceState::Running => "terminated",
                DesiredInstanceState::Terminated => "running",
            });

        filter.build()
    }
}

async fn wait_for_conforming_instances(
    ec2_client: &aws_sdk_ec2::Client,
    instance_ids: &HashSet<String>,
    desired_instance_state: DesiredInstanceState,
) -> ProviderResult<()> {
    loop {
        if !non_conforming_instances(ec2_client, instance_ids, &desired_instance_state)
            .await?
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            continue;
        }
        return Ok(());
    }
}

async fn non_conforming_instances(
    ec2_client: &aws_sdk_ec2::Client,
    instance_ids: &HashSet<String>,
    desired_instance_state: &DesiredInstanceState,
) -> ProviderResult<Vec<String>> {
    let mut describe_result = ec2_client
        .describe_instance_status()
        .filters(desired_instance_state.filter())
        .set_instance_ids(Some(Vec::from_iter(instance_ids.clone())))
        .include_all_instances(true)
        .send()
        .await
        .context(format!(
            "Unable to list instances in the '{:?}' state.",
            desired_instance_state
        ))?;
    let non_conforming_instances = describe_result
        .instance_statuses
        .as_mut()
        .context("No instance statuses were provided.")?;

    Ok(non_conforming_instances
        .iter_mut()
        .filter_map(|instance_status| instance_status.instance_id.clone())
        .collect())
}

// Find all running instances with the tag for this resource.
async fn get_instances_by_tag(ec2_client: &aws_sdk_ec2::Client) -> ProviderResult<HashSet<String>> {
    let mut describe_result = ec2_client
        .describe_instances()
        .filters(
            Filter::builder()
                .name("tag-key")
                .values(INSTANCE_TAG_NAME)
                .build(),
        )
        .send()
        .await
        .context("Unable to get instances.")?;
    let instances = describe_result
        .reservations
        .as_mut()
        .context("No instances were provided.")?;

    Ok(instances
        .iter_mut()
        // Extract the vec of `Instance`s from each `Reservation`
        .filter_map(|reservation| reservation.instances.as_ref())
        // Combine all `Instance`s into one iterator no matter which `Reservation` they
        // came from.
        .flatten()
        // Extract the instance id from each `Instance`.
        .filter_map(|instance| instance.instance_id.clone())
        .collect())
}
