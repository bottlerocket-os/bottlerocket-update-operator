/*!
  eks provider helps extracting target cluster all info, which integration test can use
  to add bottlerocket nodes to cluster and install brupop on cluster
!*/

use aws_config::meta::region::RegionProviderChain;
use aws_sdk_ec2::model::{Filter, SecurityGroup, Subnet};
use aws_sdk_ec2::Region;

use crate::error::{IntoProviderError, ProviderError, ProviderResult};

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct ClusterInfo {
    pub name: String,
    pub region: String,
    pub endpoint: String,
    pub certificate: String,
    pub public_subnet_ids: Vec<String>,
    pub private_subnet_ids: Vec<String>,
    pub nodegroup_sg: Vec<String>,
    pub controlplane_sg: Vec<String>,
    pub clustershared_sg: Vec<String>,
    pub iam_instance_profile_arn: String,
}

pub fn write_kubeconfig(
    cluster_name: &str,
    region: &str,
    kubeconfig_dir: &str,
) -> ProviderResult<()> {
    let status = Command::new("eksctl")
        .args([
            "utils",
            "write-kubeconfig",
            "-r",
            region,
            &format!("--cluster={}", cluster_name),
            &format!("--kubeconfig={}", kubeconfig_dir),
        ])
        .status()
        .context("Unable to generate and write kubeconfig")?;

    if !status.success() {
        return Err(ProviderError::new_with_context(format!(
            "Failed write kubeconfig with status code {}",
            status
        )));
    }

    Ok(())
}

pub async fn get_cluster_info(cluster_name: &str, region: &str) -> ProviderResult<ClusterInfo> {
    let region_provider = RegionProviderChain::first_try(Some(Region::new(region.to_string())));
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let eks_client = aws_sdk_eks::Client::new(&shared_config);
    let ec2_client = aws_sdk_ec2::Client::new(&shared_config);
    let iam_client = aws_sdk_iam::Client::new(&shared_config);

    let eks_subnet_ids = eks_subnet_ids(&eks_client, cluster_name).await?;
    let endpoint = endpoint(&eks_client, cluster_name).await?;
    let certificate = certificate(&eks_client, cluster_name).await?;

    let public_subnet_ids = subnet_ids(
        &ec2_client,
        cluster_name,
        eks_subnet_ids.clone(),
        SubnetType::Public,
    )
    .await?
    .into_iter()
    .filter_map(|subnet| subnet.subnet_id)
    .collect();

    let private_subnet_ids = subnet_ids(
        &ec2_client,
        cluster_name,
        eks_subnet_ids.clone(),
        SubnetType::Private,
    )
    .await?
    .into_iter()
    .filter_map(|subnet| subnet.subnet_id)
    .collect();

    let nodegroup_sg = security_group(&ec2_client, cluster_name, SecurityGroupType::NodeGroup)
        .await?
        .into_iter()
        .filter_map(|security_group| security_group.group_id)
        .collect();

    let controlplane_sg =
        security_group(&ec2_client, cluster_name, SecurityGroupType::ControlPlane)
            .await?
            .into_iter()
            .filter_map(|security_group| security_group.group_id)
            .collect();

    let clustershared_sg =
        security_group(&ec2_client, cluster_name, SecurityGroupType::ClusterShared)
            .await?
            .into_iter()
            .filter_map(|security_group| security_group.group_id)
            .collect();

    let node_instance_role = cluster_iam_identity_mapping(cluster_name, region)?;
    let iam_instance_profile_arn =
        instance_profile(&iam_client, cluster_name, &node_instance_role).await?;

    Ok(ClusterInfo {
        name: cluster_name.to_string(),
        region: region.to_string(),
        endpoint,
        certificate,
        public_subnet_ids,
        private_subnet_ids,
        nodegroup_sg,
        controlplane_sg,
        clustershared_sg,
        iam_instance_profile_arn,
    })
}

async fn eks_subnet_ids(
    eks_client: &aws_sdk_eks::Client,
    cluster_name: &str,
) -> ProviderResult<Vec<String>> {
    let describe_results = eks_client
        .describe_cluster()
        .name(cluster_name)
        .send()
        .await
        .context("Unable to get eks describe cluster")?;

    // Extract the subnet ids from the cluster.
    describe_results
        .cluster
        .as_ref()
        .context("Response missing cluster field")?
        .resources_vpc_config
        .as_ref()
        .context("Cluster missing resources_vpc_config field")?
        .subnet_ids
        .as_ref()
        .context("resources_vpc_config missing subnet ids")
        .map(|ids| ids.clone())
}

async fn endpoint(eks_client: &aws_sdk_eks::Client, cluster_name: &str) -> ProviderResult<String> {
    let describe_results = eks_client
        .describe_cluster()
        .name(cluster_name)
        .send()
        .await
        .context("Unable to get eks describe cluster")?;
    // Extract the apiserver endpoint from the cluster.
    describe_results
        .cluster
        .as_ref()
        .context("Results missing cluster field")?
        .endpoint
        .as_ref()
        .context("Cluster missing endpoint field")
        .map(|ids| ids.clone())
}

async fn certificate(
    eks_client: &aws_sdk_eks::Client,
    cluster_name: &str,
) -> ProviderResult<String> {
    let describe_results = eks_client
        .describe_cluster()
        .name(cluster_name)
        .send()
        .await
        .context("Unable to get eks describe cluster")?;

    // Extract the certificate authority from the cluster.
    describe_results
        .cluster
        .as_ref()
        .context("Results missing cluster field")?
        .certificate_authority
        .as_ref()
        .context("Cluster missing certificate_authority field")?
        .data
        .as_ref()
        .context("Certificate authority missing data")
        .map(|ids| ids.clone())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum SubnetType {
    Public,
    Private,
}

impl SubnetType {
    fn tag(&self, cluster_name: &str) -> String {
        let subnet_type = match self {
            SubnetType::Public => "Public",
            SubnetType::Private => "Private",
        };
        format!("eksctl-{}-cluster/Subnet{}*", cluster_name, subnet_type)
    }
}

async fn subnet_ids(
    ec2_client: &aws_sdk_ec2::Client,
    cluster_name: &str,
    eks_subnet_ids: Vec<String>,
    subnet_type: SubnetType,
) -> ProviderResult<Vec<Subnet>> {
    let describe_results = ec2_client
        .describe_subnets()
        .set_subnet_ids(Some(eks_subnet_ids))
        .filters(
            Filter::builder()
                .name("tag:Name")
                .values(subnet_type.tag(cluster_name))
                .build(),
        )
        .send()
        .await
        .context("Unable to get private subnet ids")?;
    describe_results
        .subnets
        .context("Results missing subnets field")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum SecurityGroupType {
    NodeGroup,
    ClusterShared,
    ControlPlane,
}

impl SecurityGroupType {
    fn tag(&self, cluster_name: &str) -> String {
        let sg = match self {
            SecurityGroupType::NodeGroup => "nodegroup",
            SecurityGroupType::ClusterShared => "ClusterSharedNodeSecurityGroup",
            SecurityGroupType::ControlPlane => "ControlPlaneSecurityGroup",
        };
        format!("*{}-{}*", cluster_name, sg)
    }
}

async fn security_group(
    ec2_client: &aws_sdk_ec2::Client,
    cluster_name: &str,
    security_group_type: SecurityGroupType,
) -> ProviderResult<Vec<SecurityGroup>> {
    // Extract the security groups.
    let describe_results = ec2_client
        .describe_security_groups()
        .filters(
            Filter::builder()
                .name("tag:Name")
                .values(security_group_type.tag(cluster_name))
                .build(),
        )
        .send()
        .await
        .context(format!(
            "Unable to get {:?} security group",
            security_group_type
        ))?;

    describe_results
        .security_groups
        .context("Results missing security_groups field")
}

async fn instance_profile(
    iam_client: &aws_sdk_iam::Client,
    cluster_name: &str,
    node_instance_role: &str,
) -> ProviderResult<String> {
    let list_result = iam_client
        .list_instance_profiles()
        .send()
        .await
        .context("Unable to list instance profiles")?;
    let eksctl_prefix = format!("eksctl-{}", cluster_name);
    list_result
        .instance_profiles
        .as_ref()
        .context("No instance profiles found")?
        .iter()
        .filter(|x| {
            x.instance_profile_name
                .as_ref()
                .unwrap_or(&"".to_string())
                .contains("NodeInstanceProfile")
        })
        .filter(|x| {
            x.instance_profile_name
                .as_ref()
                .unwrap_or(&"".to_string())
                .contains(&eksctl_prefix)
        })
        .find(|instance_profile| {
            instance_profile
                .roles
                .as_ref()
                .map(|roles| {
                    roles
                        .iter()
                        .any(|role| role.arn == Some(node_instance_role.to_string()))
                })
                .unwrap_or_default()
        })
        .context("Node instance profile not found")?
        .arn
        .as_ref()
        .context("Node instance profile missing arn field")
        .map(|profile| profile.clone())
}

fn cluster_iam_identity_mapping(cluster_name: &str, region: &str) -> ProviderResult<String> {
    let iam_identity_output = Command::new("eksctl")
        .args([
            "get",
            "iamidentitymapping",
            "--cluster",
            cluster_name,
            "--region",
            region,
            "--output",
            "json",
        ])
        .output()
        .context("Unable to get iam identity mapping.")?;

    let iam_identity: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&iam_identity_output.stdout))
            .context("Unable to deserialize iam identity mapping")?;

    iam_identity
        .get(0)
        .context("No profiles found.")?
        .get("rolearn")
        .context("Profile does not contain rolearn.")?
        .as_str()
        .context("Rolearn is not a string.")
        .map(|arn| arn.to_string())
}
