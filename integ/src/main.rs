use argh::FromArgs;
use lazy_static::lazy_static;
use log::info;
use snafu::{ensure, OptionExt, ResultExt};
use std::convert::TryFrom;
use std::env::temp_dir;
use std::fs;
use std::process;

use aws_sdk_ec2::types::ArchitectureValues;

use kube::config::{Config, KubeConfigOptions, Kubeconfig};

use integ::eks_provider::{get_cluster_info, write_kubeconfig};
use integ::error::ProviderError;
use integ::monitor::{BrupopMonitor, IntegBrupopClient, Monitor};
use integ::nodegroup_provider::{create_nodegroup, terminate_nodegroup};
use integ::updater::{
    nodes_exist, process_brupop_resources, process_cert_manager, process_pods_test, Action,
};

type Result<T> = std::result::Result<T, error::Error>;

/// The default path for kubeconfig file
const DEFAULT_KUBECONFIG_FILE_NAME: &str = "kubeconfig.yaml";

/// The default region for the cluster.
const DEFAULT_REGION: &str = "us-west-2";
const CLUSTER_NAME: &str = "brupop-integration-test";

// The default values for AMI ID
const AMI_ARCH: &str = "x86_64";

// The default name for the nodegroup
const NODEGROUP_NAME: &str = "brupop-integ-test-nodegroup";

const NAMESPACE: &str = "brupop-bottlerocket-aws";

const WAIT_CERT_MANAGER_COMPLETE: tokio::time::Duration = tokio::time::Duration::from_secs(90);

lazy_static! {
    static ref ARCHES: Vec<ArchitectureValues> =
        vec![ArchitectureValues::Arm64, ArchitectureValues::X8664];
}

#[tokio::main]
async fn main() {
    models::crypto::install_default_crypto_provider()
        .expect("Failed to configure crypto provider.");

    env_logger::init();

    if let Err(e) = run().await {
        eprintln!("{}", e);
        process::exit(1);
    }
}

#[derive(FromArgs, Debug, Clone)]
/// the brupop integration test
pub(crate) struct Arguments {
    #[argh(subcommand)]
    subcommand: SubCommand,
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand)]
enum SubCommand {
    IntegrationTest(IntegrationTestArgs),
    Monitor(MonitorArgs),
    Clean(CleanArgs),
}

#[derive(FromArgs, Debug, Clone)]
/// monitor an ongoing integration test
#[argh(subcommand, name = "monitor")]
struct MonitorArgs {
    /// name of the cluster that tests run in
    #[argh(option, default = "CLUSTER_NAME.to_string()")]
    cluster_name: String,

    /// the region that the cluster is in
    #[argh(option, default = "DEFAULT_REGION.to_string()")]
    region: String,

    /// path to the kubeconfig for this cluster
    #[argh(option, default = "DEFAULT_KUBECONFIG_FILE_NAME.to_string()")]
    kube_config_path: String,
}

#[derive(FromArgs, Debug, Clone)]
/// clean up nodegroups after testing
#[argh(subcommand, name = "clean")]
struct CleanArgs {
    /// name of the cluster that tests run in
    #[argh(option, default = "CLUSTER_NAME.to_string()")]
    cluster_name: String,

    /// the region that the cluster is in
    #[argh(option, default = "DEFAULT_REGION.to_string()")]
    region: String,

    /// the nodegroup to create for the test
    #[argh(option, default = "NODEGROUP_NAME.to_string()")]
    nodegroup_name: String,

    /// path to the kubeconfig for this cluster
    #[argh(option, default = "DEFAULT_KUBECONFIG_FILE_NAME.to_string()")]
    kube_config_path: String,
}

#[derive(FromArgs, Debug, Clone)]
/// starts integration tests
#[argh(subcommand, name = "integration-test")]
pub struct IntegrationTestArgs {
    /// name of the cluster that tests run in
    #[argh(option, default = "CLUSTER_NAME.to_string()")]
    cluster_name: String,

    /// the region that the cluster is in
    #[argh(option, default = "DEFAULT_REGION.to_string()")]
    region: String,

    /// the nodegroup to create for the test
    #[argh(option, default = "NODEGROUP_NAME.to_string()")]
    nodegroup_name: String,

    /// path to the kubeconfig for this cluster
    #[argh(option, default = "DEFAULT_KUBECONFIG_FILE_NAME.to_string()")]
    kube_config_path: String,

    /// the version of bottlerocket to test
    #[argh(option)]
    bottlerocket_version: String,

    /// the architecture of the given AMI
    #[argh(option, default = "AMI_ARCH.to_string()")]
    ami_arch: String,
}

/// All subcommands have a few common arguments, but `argh` doesn't support hoisting these into a "global" struct in
/// the same way that e.g. `clap` does. So we implement a "global" struct and some conversions here.
mod commonargs {
    use super::*;

    /// Arguments shared by most subcommands
    #[derive(Debug, Clone)]
    pub struct CommonArgs {
        pub cluster_name: String,
        pub region: String,
        pub kube_config_path: String,
    }

    impl From<&Arguments> for CommonArgs {
        fn from(arguments: &Arguments) -> Self {
            match &arguments.subcommand {
                SubCommand::IntegrationTest(args) => args.into(),
                SubCommand::Monitor(args) => args.into(),
                SubCommand::Clean(args) => args.into(),
            }
        }
    }

    impl From<&MonitorArgs> for CommonArgs {
        fn from(monitor_args: &MonitorArgs) -> Self {
            Self {
                cluster_name: monitor_args.cluster_name.clone(),
                region: monitor_args.region.clone(),
                kube_config_path: monitor_args.kube_config_path.clone(),
            }
        }
    }
    impl From<&CleanArgs> for CommonArgs {
        fn from(clean_args: &CleanArgs) -> Self {
            Self {
                cluster_name: clean_args.cluster_name.clone(),
                region: clean_args.region.clone(),
                kube_config_path: clean_args.kube_config_path.clone(),
            }
        }
    }
    impl From<&IntegrationTestArgs> for CommonArgs {
        fn from(integ_args: &IntegrationTestArgs) -> Self {
            Self {
                cluster_name: integ_args.cluster_name.clone(),
                region: integ_args.region.clone(),
                kube_config_path: integ_args.kube_config_path.clone(),
            }
        }
    }
}
use commonargs::CommonArgs;

async fn generate_kubeconfig(arguments: &CommonArgs) -> Result<String> {
    // default kube config path is /temp/{CLUSTER_NAME}-{REGION}/kubeconfig.yaml
    let kube_config_path = generate_kubeconfig_file_path(arguments).await?;

    // decode and write kubeconfig
    info!("decoding and writing kubeconfig ...");

    write_kubeconfig(
        &arguments.cluster_name,
        &arguments.region,
        &kube_config_path,
    )
    .context(error::WriteKubeconfigSnafu)?;
    info!(
        "kubeconfig has been written and store at {:?}",
        &kube_config_path
    );

    Ok(kube_config_path)
}

async fn generate_kubeconfig_file_path(arguments: &CommonArgs) -> Result<String> {
    let unique_kube_config_temp_dir = get_kube_config_temp_dir_path(arguments)?;

    fs::create_dir_all(&unique_kube_config_temp_dir).context(error::CreateDirSnafu)?;

    let kube_config_path = format!(
        "{}/{}",
        &unique_kube_config_temp_dir, DEFAULT_KUBECONFIG_FILE_NAME
    );

    Ok(kube_config_path)
}

fn get_kube_config_temp_dir_path(arguments: &CommonArgs) -> Result<String> {
    let unique_tmp_dir_name = format!("{}-{}", arguments.cluster_name, arguments.region);
    let unique_kube_config_temp_dir = format!(
        "{}/{}",
        temp_dir().to_str().context(error::FindTmpDirSnafu)?,
        unique_tmp_dir_name
    );

    Ok(unique_kube_config_temp_dir)
}

fn args_validation(args: &Arguments) -> Result<()> {
    match &args.subcommand {
        SubCommand::IntegrationTest(integ_test_args) => {
            ensure!(
                ARCHES.contains(&ArchitectureValues::from(integ_test_args.ami_arch.as_str())),
                error::InvalidArchInputSnafu {
                    input: integ_test_args.ami_arch.clone()
                }
            )
        }
        _ => return Ok(()),
    }
    Ok(())
}

async fn run() -> Result<()> {
    // Parse and store the args passed to the program
    let args: Arguments = argh::from_env();

    // Validate the args
    args_validation(&args)?;

    let subcommand = &args.subcommand;
    let args: CommonArgs = (&args).into();

    let cluster_info = get_cluster_info(&args.cluster_name, &args.region)
        .await
        .context(error::GetClusterInfoSnafu)?;

    match subcommand {
        SubCommand::IntegrationTest(integ_test_args) => {
            // Generate kubeconfig if no input value for argument `kube_config_path`
            let kube_config_path: String = match args.kube_config_path.as_str() {
                DEFAULT_KUBECONFIG_FILE_NAME => generate_kubeconfig(&args).await?,
                res => res.to_string(),
            };

            // Create instances via nodegroup and add nodes to eks cluster
            info!("Creating EC2 instances via nodegroup ...");
            create_nodegroup(
                cluster_info,
                &integ_test_args.nodegroup_name,
                &integ_test_args.ami_arch,
                &integ_test_args.bottlerocket_version,
            )
            .await
            .context(error::CreateNodeGroupSnafu)?;
            info!("EC2 instances/nodegroup have been created");

            // create different types' pods to test if brupop can handle them.
            info!(
                "creating pods(statefulset pods, stateless pods, and pods with PodDisruptionBudgets) ...
            "
            );
            process_pods_test(Action::Apply, &kube_config_path)
                .await
                .context(error::CreatePodSnafu)?;

            // install cert-manager and brupop on EKS cluster
            info!("Running cert-manager on existing EKS cluster ...");
            process_cert_manager(Action::Apply, &kube_config_path)
                .await
                .context(error::RunBrupopSnafu)?;
            tokio::time::sleep(WAIT_CERT_MANAGER_COMPLETE).await;
            info!("Running brupop on existing EKS cluster ...");
            process_brupop_resources(Action::Apply, &kube_config_path)
                .await
                .context(error::RunBrupopSnafu)?;
        }
        SubCommand::Monitor(_) => {
            // generate kubeconfig path if no input value for argument `kube_config_path`
            let kube_config_path: String = match args.kube_config_path.as_str() {
                DEFAULT_KUBECONFIG_FILE_NAME => generate_kubeconfig_file_path(&args).await?,
                res => res.to_string(),
            };

            // create k8s client
            let kubeconfig =
                Kubeconfig::read_from(kube_config_path).context(error::ReadKubeConfigSnafu)?;
            let config = Config::from_custom_kubeconfig(
                kubeconfig.to_owned(),
                &KubeConfigOptions::default(),
            )
            .await
            .context(error::LoadKubeConfigSnafu)?;

            let k8s_client =
                kube::client::Client::try_from(config).context(error::CreateK8sClientSnafu)?;

            info!("monitoring brupop");
            let monitor_client = BrupopMonitor::new(IntegBrupopClient::new(k8s_client, NAMESPACE));
            monitor_client
                .run_monitor()
                .await
                .context(error::MonitorBrupopSnafu)?;
        }
        SubCommand::Clean(clean_args) => {
            // Generate kubeconfig path if no input value for argument `kube_config_path`
            let kube_config_path: String = match args.kube_config_path.as_str() {
                DEFAULT_KUBECONFIG_FILE_NAME => generate_kubeconfig_file_path(&args).await?,
                res => res.to_string(),
            };

            // Create k8s client
            let kubeconfig =
                Kubeconfig::read_from(&kube_config_path).context(error::ReadKubeConfigSnafu)?;
            let config = Config::from_custom_kubeconfig(
                kubeconfig.to_owned(),
                &KubeConfigOptions::default(),
            )
            .await
            .context(error::LoadKubeConfigSnafu)?;
            let k8s_client =
                kube::client::Client::try_from(config).context(error::CreateK8sClientSnafu)?;

            // Terminate nodegroup created by integration test.
            info!("Terminating nodegroup ...");
            terminate_nodegroup(cluster_info, &clean_args.nodegroup_name)
                .await
                .context(error::TerminateNodeGroupSnafu)?;

            // If EKS cluster still has running nodes which need brupop, Integration-test shouldn't uninstall brupop, delete test pods, and kubeconfig file.
            if !nodes_exist(k8s_client)
                .await
                .context(error::RunBrupopSnafu)?
            {
                // Clean up cert-manager and all brupop resources like namespace, deployment on brupop test
                info!("Deleting all brupop cluster resources created by integration test ...");
                process_brupop_resources(Action::Delete, &kube_config_path)
                    .await
                    .context(error::DeleteClusterResourcesSnafu)?;
                info!("Deleting cert-manager created by integration test ...");
                process_cert_manager(Action::Delete, &kube_config_path)
                    .await
                    .context(error::DeleteClusterResourcesSnafu)?;

                // delete all created pods for testing.
                info!(
                "deleting pods(statefulset pods, stateless pods, and pods with PodDisruptionBudgets) ...
            "
            );
                process_pods_test(Action::Delete, &kube_config_path)
                    .await
                    .context(error::DeletePodSnafu)?;

                // Delete tmp directory and kubeconfig.yaml if no input value for argument `kube_config_path`
                if args.kube_config_path == DEFAULT_KUBECONFIG_FILE_NAME {
                    info!("Deleting tmp directory and kubeconfig.yaml ...");
                    fs::remove_dir_all(get_kube_config_temp_dir_path(&args)?)
                        .context(error::DeleteTmpDirSnafu)?;
                }
            }
        }
    }
    Ok(())
}

mod error {
    use crate::ProviderError;
    use integ::monitor::monitor_error;
    use integ::updater::update_error;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(super) enum Error {
        #[snafu(display("Failed to get eks cluster info: {}", source))]
        GetClusterInfo { source: ProviderError },

        #[snafu(display("Unable to create directory for storing kubeconfig file: {}", source))]
        CreateDir { source: std::io::Error },

        #[snafu(display("Failed to create pods on eks cluster: {}", source))]
        CreatePod { source: update_error::Error },

        #[snafu(display("Failed to delete pods on eks cluster: {}", source))]
        DeletePod { source: update_error::Error },

        #[snafu(display("Invalid Arch input: {}", input))]
        InvalidArchInput { input: String },

        #[snafu(display("Unable create K8s client from kubeconfig: {}", source))]
        CreateK8sClient { source: kube::Error },

        #[snafu(display("Failed to create node group: {}", source))]
        CreateNodeGroup { source: ProviderError },

        #[snafu(display("Unable load kubeconfig: {}", source))]
        LoadKubeConfig {
            source: kube::config::KubeconfigError,
        },

        #[snafu(display("Unable to read kubeconfig: {}", source))]
        ReadKubeConfig {
            source: kube::config::KubeconfigError,
        },

        #[snafu(display("Failed to install brupop on eks cluster: {}", source))]
        RunBrupop { source: update_error::Error },

        #[snafu(display("Failed to monitor brupop on eks cluster: {}", source))]
        MonitorBrupop { source: monitor_error::Error },

        #[snafu(display("Failed to terminate node group: {}", source))]
        TerminateNodeGroup { source: ProviderError },

        #[snafu(display("Failed to delete created eks cluster resources: {}", source))]
        DeleteClusterResources { source: update_error::Error },

        #[snafu(display("Failed to delete tmp directory and kubeconfig.yaml: {}", source))]
        DeleteTmpDir { source: std::io::Error },

        #[snafu(display("Unable to find temp directory"))]
        FindTmpDir {},

        #[snafu(display("Failed to write content to kubeconfig: {}", source))]
        WriteKubeconfig { source: ProviderError },
    }
}
