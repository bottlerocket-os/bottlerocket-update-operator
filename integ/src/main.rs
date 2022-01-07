use log::info;
use snafu::{OptionExt, ResultExt};
use std::env::temp_dir;
use std::fs;
use std::process;
use structopt::StructOpt;
use tokio::time::{sleep, Duration};

use integ::ec2_provider::{create_ec2_instance, terminate_ec2_instance};
use integ::eks_provider::{get_cluster_info, write_kubeconfig};
use integ::error::ProviderError;
use integ::updater::{delete_cluster_resources, label_node, run_brupop};

type Result<T> = std::result::Result<T, error::Error>;

/// The default path for kubeconfig file
const DEFAULT_KUBECONFIG_FILE_NAME: &str = "kubeconfig.yaml";

/// The default region for the cluster.
const DEFAULT_REGION: &str = "us-west-2";
const CLUSTER_NAME: &str = "brupop-integration-test";

/// This value configure how long it sleeps between create instance and label instance.
const INTEGRATION_TEST_DELAY: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    env_logger::init();

    if let Err(e) = run().await {
        eprintln!("{}", e);
        process::exit(1);
    }
}

#[derive(StructOpt, Debug)]
pub(crate) struct Arguments {
    #[structopt(global = true, long = "--cluster-name", default_value = CLUSTER_NAME)]
    cluster_name: String,

    #[structopt(global = true, long = "--region", default_value = DEFAULT_REGION)]
    region: String,

    #[structopt(
        long = "--kube-config-path",
        default_value = DEFAULT_KUBECONFIG_FILE_NAME
    )]
    kube_config_path: String,

    #[structopt(subcommand)]
    subcommand: SubCommand,
}

#[derive(StructOpt, Debug)]
enum SubCommand {
    IntegrationTest(IntegrationTestArgs),
    Clean,
}

// Stores user-supplied arguments for the 'integration-test' subcommand.
#[derive(StructOpt, Debug)]
struct IntegrationTestArgs {
    #[structopt(long = "--instance-ami-id")]
    instance_ami_id: String,
}

async fn generate_kube_config_path(args_kube_config_path: &str) -> Result<String> {
    // default kube config path is /temp/kubeconfig.yaml
    let kube_config_path = match args_kube_config_path {
        DEFAULT_KUBECONFIG_FILE_NAME => format!(
            "{}/{}",
            temp_dir().to_str().context(error::FindTmpDir)?,
            DEFAULT_KUBECONFIG_FILE_NAME
        ),
        res => res.to_string(),
    };

    Ok(kube_config_path)
}

async fn run() -> Result<()> {
    // Parse and store the args passed to the program
    let args = Arguments::from_args();

    // generate kube config path if no input value for argument `kube_config_path`
    let kube_config_path = generate_kube_config_path(&args.kube_config_path).await?;

    let cluster_info = get_cluster_info(&args.cluster_name, &args.region)
        .await
        .context(error::GetClusterInfo)?;

    match &args.subcommand {
        SubCommand::IntegrationTest(integ_test_args) => {
            // create instances and add nodes to eks cluster
            info!("Creating EC2 instances ...");
            let created_instances =
                create_ec2_instance(cluster_info, &integ_test_args.instance_ami_id)
                    .await
                    .context(error::CreateEc2Instances)?;
            info!("EC2 instances have been created");

            // decode and write kubeconfig
            info!("decoding and writing kubeconfig ...");

            write_kubeconfig(&args.cluster_name, &DEFAULT_REGION, &kube_config_path)
                .context(error::WriteKubeconfig)?;
            info!(
                "kubeconfig has been written and store at {:?}",
                kube_config_path
            );

            info!("Sleeping 60 secs to wait Nodes be ready ...");
            sleep(INTEGRATION_TEST_DELAY).await;

            // label created nodes. To start Bottlerocket updater operator agent on your nodes,
            // you need to add the bottlerocket.aws/updater-interface-version label
            info!(
                "labeling ec2 instances (nodes) ...
            "
            );
            label_node(created_instances.private_dns_name, &kube_config_path)
                .await
                .context(error::LabelNode)?;
            info!("EC2 instances (nodes) have been labelled");

            // install brupop on EKS cluster
            info!("Running brupop on existing EKS cluster ...");
            run_brupop(&kube_config_path)
                .await
                .context(error::RunBrupop)?;
        }
        SubCommand::Clean => {
            // terminate all instances which created by integration test.
            info!("Terminating EC2 instance ...");
            terminate_ec2_instance(cluster_info)
                .await
                .context(error::TerminateEc2Instance)?;

            // clean up all resources like namespace, deployment on brupop test
            info!("Deleting all cluster resources which created by integration test ...");
            delete_cluster_resources(&kube_config_path)
                .await
                .context(error::DeleteClusterResources)?;

            // delete tmp directory and kubeconfig.yaml
            info!("Deleting tmp directory and kubeconfig.yaml ...");
            fs::remove_file(kube_config_path).context(error::DeleteTmpDir)?;
        }
    }
    Ok({})
}

mod error {
    use crate::ProviderError;
    use integ::updater::update_error;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub(super)")]
    pub(super) enum Error {
        #[snafu(display("Failed to get eks cluster info: {}", source))]
        GetClusterInfo { source: ProviderError },

        #[snafu(display("Failed to create ec2 instances: {}", source))]
        CreateEc2Instances { source: ProviderError },

        #[snafu(display("Failed to label ec2 instances: {}", source))]
        LabelNode { source: update_error::Error },

        #[snafu(display("Failed to install brupop on eks cluster: {}", source))]
        RunBrupop { source: update_error::Error },

        #[snafu(display("Failed to terminate ec2 instances: {}", source))]
        TerminateEc2Instance { source: ProviderError },

        #[snafu(display("Failed to delete created eks cluster resources: {}", source))]
        DeleteClusterResources { source: update_error::Error },

        #[snafu(display("Failed to delete tmp directory and kubeconfig.yaml: {}", source))]
        DeleteTmpDir { source: std::io::Error },

        #[snafu(display("Unable to find temp directory"))]
        FindTmpDir {},

        #[snafu(display("Failed to write content to kubeconfig: {}", source))]
        WriteKubeconfig { source: ProviderError },

        #[snafu(display("Logger setup error: {}", source))]
        Logger { source: log::SetLoggerError },
    }
}
