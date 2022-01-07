/*!
  updater helps running brupop on existing EKS cluster and clean up all
  resources once completing integration test
!*/

use lazy_static::lazy_static;
use snafu::{ensure, ResultExt};
use std::process::Command;

use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

const BRUPOP_NODE_LABEL: &str = "bottlerocket.aws/updater-interface-version=2.0.0";
const BRUPOP_NAMESPACE: &str = "brupop-bottlerocket-aws";

lazy_static! {
    static ref BRUPOP_CLUSTER_ROLES: Vec<&'static str> = {
        let mut m = Vec::new();
        m.push("brupop-apiserver-role");
        m.push("brupop-agent-role");
        m.push("brupop-controller-role");
        m
    };
}

lazy_static! {
    static ref BRUPOP_CLUSTER_ROLE_BINDINGS: Vec<&'static str> = {
        let mut m = Vec::new();
        m.push("brupop-apiserver-role-binding");
        m.push("brupop-agent-role-binding");
        m.push("brupop-controller-role-binding");
        m
    };
}

// The reflector uses exponential backoff.
// These values configure how long to delay between tries.
const RETRY_BASE_DELAY: Duration = Duration::from_secs(20);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(60);
const NUM_RETRIES: usize = 5;

// label node with `bottlerocket.aws/updater-interface-version=2.0.0`, so brupop can find right nodes
// to run updater
pub async fn label_node(node_names: Vec<String>, kube_config_path: &str) -> UpdaterResult<()> {
    // When integration test creates and add nodes to EKS cluster, nodes usually need few mins to be ready.
    // Label process will be failed if nodes aren't ready. Therefore, add retry strategy here to avoid this situation,
    // and give nodes more time to be ready.
    Retry::spawn(retry_strategy(), || async {
        for node in &node_names {
            let status = Command::new("kubectl")
                .args([
                    "label",
                    "node",
                    node,
                    BRUPOP_NODE_LABEL,
                    "--kubeconfig",
                    kube_config_path,
                    "--overwrite=true",
                ])
                .status()
                .context(update_error::LabelNode)?;
            if status.success() {
                continue;
            } else {
                return Err(update_error::Error::BrupopRun);
            }
        }
        Ok(())
    })
    .await
}

// installing brupop on EKS cluster
pub async fn run_brupop(kube_config_path: &str) -> UpdaterResult<()> {
    let brn_status = Command::new("kubectl")
        .args([
            "apply",
            "-f",
            "yamlgen/deploy/bottlerocket-node-crd.yaml",
            "--kubeconfig",
            kube_config_path,
        ])
        .status()
        .context(update_error::BrupopProcess)?;

    ensure!(brn_status.success(), update_error::BrupopRun);

    let brupop_resource_status = Command::new("kubectl")
        .args([
            "apply",
            "-f",
            "yamlgen/deploy/brupop-resources.yaml",
            "--kubeconfig",
            kube_config_path,
        ])
        .status()
        .context(update_error::BrupopProcess)?;

    ensure!(brupop_resource_status.success(), update_error::BrupopRun);

    Ok(())
}

fn retry_strategy() -> impl Iterator<Item = Duration> {
    ExponentialBackoff::from_millis(RETRY_BASE_DELAY.as_millis() as u64)
        .max_delay(RETRY_MAX_DELAY)
        .map(jitter)
        .take(NUM_RETRIES)
}

// destroy all resources which were created when integration test installed brupop
pub async fn delete_cluster_resources(kube_config_path: &str) -> UpdaterResult<()> {
    // delete namespaces brupop-bottlerocket-aws. This can clean all resources under this namespace like daemonsets.apps
    let namespace_deletion_status = Command::new("kubectl")
        .args([
            "delete",
            "namespaces",
            BRUPOP_NAMESPACE,
            "--kubeconfig",
            kube_config_path,
        ])
        .status()
        .context(update_error::BrupopCleanUp {
            cluster_resource: "namespaces",
        })?;
    ensure!(namespace_deletion_status.success(), update_error::BrupopRun);

    // delete clusterrolebinding.rbac.authorization.k8s.io
    for cluster_role_binding in BRUPOP_CLUSTER_ROLE_BINDINGS.iter() {
        let clusterrolebinding_deletion_status = Command::new("kubectl")
            .args([
                "delete",
                "clusterrolebinding.rbac.authorization.k8s.io",
                cluster_role_binding,
                "--kubeconfig",
                kube_config_path,
            ])
            .status()
            .context(update_error::BrupopCleanUp {
                cluster_resource: "clusterrolebinding.rbac.authorization.k8s.io",
            })?;
        ensure!(
            clusterrolebinding_deletion_status.success(),
            update_error::BrupopRun
        );
    }

    // delete clusterrole.rbac.authorization.k8s.io
    for cluster_role in BRUPOP_CLUSTER_ROLES.iter() {
        let clusterrole_deletion_status = Command::new("kubectl")
            .args([
                "delete",
                "clusterrole.rbac.authorization.k8s.io",
                cluster_role,
                "--kubeconfig",
                kube_config_path,
            ])
            .status()
            .context(update_error::BrupopCleanUp {
                cluster_resource: "clusterrole.rbac.authorization.k8s.io",
            })?;
        ensure!(
            clusterrole_deletion_status.success(),
            update_error::BrupopRun
        );
    }

    Ok(())
}

/// The result type returned by instance create and termiante operations.
type UpdaterResult<T> = std::result::Result<T, update_error::Error>;

pub mod update_error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum Error {
        #[snafu(display("Failed to label node: {}", source))]
        LabelNode { source: std::io::Error },

        #[snafu(display("Failed to install brupop: {}", source))]
        BrupopProcess { source: std::io::Error },

        #[snafu(display("Failed to run brupop test"))]
        BrupopRun,

        #[snafu(display("Failed to deleted resource {}: {}", cluster_resource, source))]
        BrupopCleanUp {
            cluster_resource: String,
            source: std::io::Error,
        },
        #[snafu(display("Unable to convert kubeconfig path to string path"))]
        ConvertPathToStr {},
    }
}
