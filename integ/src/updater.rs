/*!
  updater helps running brupop on existing EKS cluster and clean up all
  resources once completing integration test
!*/

use lazy_static::lazy_static;
use snafu::{ensure, ResultExt};
use std::process::Command;

use k8s_openapi::api::core::v1::Node;
use kube::api::{Api, ListParams};

use models::constants::NAMESPACE;

const CURRENT_LOCATION_PATH: &str = "integ/src";
const PODS_TEMPLATE: &str = "pods-template.yaml";
const KUBECTL_BINARY: &str = "kubectl";

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

#[derive(strum_macros::Display, Debug)]
pub enum Action {
    Apply,
    Delete,
}

// installing brupop on EKS cluster
pub async fn run_brupop(kube_config_path: &str) -> UpdaterResult<()> {
    let brupop_resource_status = Command::new(KUBECTL_BINARY)
        .args([
            "apply",
            "-f",
            "yamlgen/deploy/bottlerocket-update-operator.yaml",
            "--kubeconfig",
            kube_config_path,
        ])
        .status()
        .context(update_error::BrupopProcess)?;

    ensure!(brupop_resource_status.success(), update_error::BrupopRun);

    Ok(())
}

// destroy all brupop resources which were created when integration test installed brupop
pub async fn delete_brupop_cluster_resources(kube_config_path: &str) -> UpdaterResult<()> {
    // delete namespaces brupop-bottlerocket-aws. This can clean all resources under this namespace like daemonsets.apps
    let namespace_deletion_status = Command::new("kubectl")
        .args([
            "delete",
            "namespaces",
            NAMESPACE,
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

// =^..^=   =^..^=   =^..^=   =^..^=   =^..^= Deletion and Creation of test pods  =^..^=   =^..^=   =^..^=   =^..^=   =^..^=

// create or delete statefulset pods, stateless nginx pods, and pods with PDBs on EKS cluster
pub async fn process_pods_test(action: Action, kube_config_path: &str) -> UpdaterResult<()> {
    let action_string: String = action.to_string();

    let pods_status = Command::new(KUBECTL_BINARY)
        .args([
            &action_string.to_lowercase(),
            "-f",
            format!("{}/{}", CURRENT_LOCATION_PATH, PODS_TEMPLATE).as_str(),
            "--kubeconfig",
            kube_config_path,
        ])
        .status()
        .context(update_error::ProcessPodsTest {
            action: action_string.clone(),
        })?;

    ensure!(
        pods_status.success(),
        update_error::PodsRun {
            action: action_string
        }
    );
    Ok(())
}

// Find if any node is running in the cluster
pub async fn nodes_exist(k8s_client: kube::client::Client) -> UpdaterResult<bool> {
    let nodes: Api<Node> = Api::all(k8s_client.clone());

    let nodes_objectlist = nodes
        .list(&ListParams::default())
        .await
        .context(update_error::FindNodes {})?;

    Ok(nodes_objectlist.iter().count() > 0)
}

/// The result type returned by instance create and termiante operations.
type UpdaterResult<T> = std::result::Result<T, update_error::Error>;

pub mod update_error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum Error {
        #[snafu(display("Failed to install brupop: {}", source))]
        BrupopProcess { source: std::io::Error },

        #[snafu(display("Failed to run brupop test"))]
        BrupopRun,

        #[snafu(display("Failed to deleted resource {}: {}", cluster_resource, source))]
        BrupopCleanUp {
            cluster_resource: String,
            source: std::io::Error,
        },

        #[snafu(display("Failed to {:?} pods", action))]
        ProcessPodsTest {
            action: String,
            source: std::io::Error,
        },

        #[snafu(display("Failed to process pods test: {:?} pods", action))]
        PodsRun { action: String },

        #[snafu(display("Unable to convert kubeconfig path to string path"))]
        ConvertPathToStr {},

        #[snafu(display("Fail to list EKS cluster nodes: {}", source))]
        FindNodes { source: kube::Error },
    }
}
