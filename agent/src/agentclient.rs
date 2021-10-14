use chrono::naive::NaiveDateTime;
use k8s_openapi::api::core::v1::Node;
use kube::Api;
use snafu::OptionExt;
use snafu::ResultExt;
use std::env;
use std::process::Command;
use tokio::time::{sleep, Duration};

use crate::apiclient::{boot_update, get_os_info, list_available, prepare, update};
use crate::error::{self, Result};
use apiserver::api::{CreateBottlerocketNodeRequest, UpdateBottlerocketNodeRequest};
use models::constants::{APISERVER_SERVICE_NAME, APISERVER_SERVICE_PORT, NAMESPACE};
use models::node::{
    node_resource_name, BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeState,
    BottlerocketNodeStatus,
};

const AGENT_SLEEP_DURATION: Duration = Duration::from_secs(5);
const NODE_RESOURCE_ENDPOINT: &'static str = "/bottlerocket-node-resource";

#[derive(Clone)]
pub struct BrupopAgent {
    k8s_client: kube::client::Client,
    node_selector: Option<BottlerocketNodeSelector>,
}

impl BrupopAgent {
    pub fn new(k8s_client: kube::client::Client) -> Self {
        BrupopAgent {
            k8s_client,
            node_selector: None,
        }
    }

    pub async fn check_node_custom_resource_exists(&mut self) -> Result<bool> {
        let associated_bottlerocketnode_name = node_resource_name(&self.get_node_selector().await?);
        let bottlerocket_nodes: Api<BottlerocketNode> =
            Api::namespaced(self.k8s_client.clone(), NAMESPACE);

        Ok(bottlerocket_nodes
            .get(&associated_bottlerocketnode_name)
            .await
            .context(error::BottlerocketNodeNotExist {
                node_name: associated_bottlerocketnode_name,
            })
            .is_ok())
    }

    pub async fn check_custom_resource_status_exists(&mut self) -> Result<bool> {
        Ok(self.fetch_custom_resource().await?.status.is_some())
    }

    async fn get_node_selector(&mut self) -> Result<BottlerocketNodeSelector> {
        if self.node_selector.is_none() {
            let associated_node_name = env::var("MY_NODE_NAME").context(error::GetNodeName)?;

            let nodes: Api<Node> = Api::all(self.k8s_client.clone());
            let associated_node: Node = nodes
                .get(&associated_node_name)
                .await
                .context(error::FetchNode)?;

            let associated_node_uid = associated_node
                .metadata
                .uid
                .context(error::MissingNodeUid)?;
            self.node_selector = Some(BottlerocketNodeSelector {
                node_name: associated_node_name.clone(),
                node_uid: associated_node_uid.clone(),
            });
        }

        Ok(self
            .node_selector
            .clone()
            .context(error::NodeSelectorIsNone)?)
    }

    // fetch associated bottlerocketnode (custom resource) and help to get node `metadata`, `spec`, and  `status`.
    async fn fetch_custom_resource(&mut self) -> Result<BottlerocketNode> {
        // get associated bottlerocketnode (custom resource) name, and we can use this
        // bottlerocketnode name to fetch targeted bottlerocketnode (custom resource) and get node `metadata`, `spec`, and  `status`.

        //TODO: use a reflector to watch event instead of polling etcd
        let associated_bottlerocketnode_name =
            format!("brn-{}", self.get_node_selector().await?.node_name);

        let bottlerocket_nodes: Api<BottlerocketNode> =
            Api::namespaced(self.k8s_client.clone(), NAMESPACE);
        let associated_bottlerocketnode: BottlerocketNode = bottlerocket_nodes
            .get(&associated_bottlerocketnode_name)
            .await
            .context(error::FetchCustomResource)?;

        Ok(associated_bottlerocketnode)
    }

    /// Gather metadata about the system, node status provided by spec.
    async fn gather_system_metadata(
        &mut self,
        state: BottlerocketNodeState,
    ) -> Result<BottlerocketNodeStatus> {
        let current_version = get_os_info()
            .await
            .context(error::BottlerocketNodeStatusVersion)?;
        let update_versions = list_available()
            .await
            .context(error::BottlerocketNodeStatusAvailableVersions)?;

        Ok(BottlerocketNodeStatus {
            current_version: current_version.version_id.clone(),
            available_versions: update_versions,
            current_state: state,
        })
    }

    /// create the custom resource associated with this node
    pub async fn create_metadata_custom_resource(&mut self) -> Result<()> {
        let client = reqwest::Client::new();
        let current_node_selector = self.get_node_selector().await?;
        let node_req = CreateBottlerocketNodeRequest {
            node_selector: current_node_selector.clone(),
        };
        let server_domain = get_server_domain();

        let response = client
            .post(format!(
                "http://{}{}",
                &server_domain, NODE_RESOURCE_ENDPOINT
            ))
            .json(&node_req)
            .send()
            .await
            .context(error::CreateBottlerocketNodeResource {
                node_name: current_node_selector.node_name,
            })?;

        log::info!(
            "{}",
            response
                .text()
                .await
                .context(error::ConvertResponseToText)?
        );

        Ok(())
    }

    /// update the custom resource associated with this node
    async fn update_metadata_custom_resource(
        &mut self,
        _current_metadata: BottlerocketNodeStatus,
    ) -> Result<()> {
        let client = reqwest::Client::new();
        let current_node_selector = self.get_node_selector().await?;
        let server_domain = get_server_domain();

        let node_req = UpdateBottlerocketNodeRequest {
            node_status: _current_metadata,
            node_selector: current_node_selector.clone(),
        };

        let response = client
            .put(format!(
                "http://{}{}",
                &server_domain, NODE_RESOURCE_ENDPOINT
            ))
            .json(&node_req)
            .send()
            .await
            .context(error::UpdateBottlerocketNodeResource {
                node_name: current_node_selector.node_name,
            })?;

        log::info!(
            "{}",
            response
                .text()
                .await
                .context(error::ConvertResponseToText)?
        );

        Ok(())
    }

    /// initialize bottlerocketnode (custom resource) `status` when create new bottlerocketnode
    pub async fn initialize_metadata_custom_resource(&mut self) -> Result<()> {
        let update_node_status = self
            .gather_system_metadata(BottlerocketNodeState::WaitingForUpdate)
            .await?;

        self.update_metadata_custom_resource(update_node_status)
            .await?;
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        // A running agent has two responsibilities:
        // - Gather metadata about the system and update the custom resource associated with this node
        // - Determine if the spec on the system's custom resource demands the node take action. If so, begin taking that action.

        loop {
            // Requests metadata 'status' and 'spec' for the current BottlerocketNode
            let bottlerocket_node = self.fetch_custom_resource().await?;

            let bottlerocket_node_status = bottlerocket_node
                .status
                .context(error::MissingBottlerocketNodeStatus)?;
            let bottlerocket_node_spec = bottlerocket_node.spec;

            // Determine if the spec on the system's custom resource demands the node take action. If so, begin taking that action.
            if bottlerocket_node_spec.state != bottlerocket_node_status.current_state {
                log::info!("Detected drift between spec state and current state. Requesting node to take action: {:?}.", &bottlerocket_node_spec.state);

                match bottlerocket_node_spec.state {
                    BottlerocketNodeState::WaitingForUpdate => {
                        log::info!("Ready to finish monitoring and start update process")
                    }
                    BottlerocketNodeState::PreparingToUpdate => {
                        log::info!("Preparing update");
                        prepare().await.context(error::UpdateActions {
                            action: "Prepare".to_string(),
                        })?;

                        // TODO: This function needs to use the k8s drain API to remove any pods from the host,
                        // and then we need to wait until the host is successfully drained before transitioning
                        // to the next state.
                    }
                    BottlerocketNodeState::PerformingUpdate => {
                        log::info!("Performing update");
                        update().await.context(error::UpdateActions {
                            action: "Perform".to_string(),
                        })?;
                    }
                    BottlerocketNodeState::RebootingToUpdate => {
                        log::info!("Rebooting node to complete update");

                        if ensure_reboot_happened(&bottlerocket_node_status)? {
                            // previous execution `reboot` exited loop and did not update the custom resource
                            // associated with this node. When re-enter loop and finished reboot,
                            // try to update the custom resource.
                            let update_node_status = self
                                .gather_system_metadata(bottlerocket_node_spec.state.clone())
                                .await?;
                            self.update_metadata_custom_resource(update_node_status)
                                .await?;
                        } else {
                            boot_update().await.context(error::UpdateActions {
                                action: "Reboot".to_string(),
                            })?;
                        }
                    }
                    BottlerocketNodeState::MonitoringUpdate => {
                        log::info!("Monitoring node's healthy condtion");
                        // TODO: we need add some critierias here by which we decide to transition
                        // from MonitoringUpdate to WaitingForUpdate.
                    }
                }
            } else {
                log::info!("Did not detect action demand.");
            }

            // update the custom resource associated with this node
            let update_node_status = self
                .gather_system_metadata(bottlerocket_node_spec.state.clone())
                .await?;
            self.update_metadata_custom_resource(update_node_status)
                .await?;

            log::debug!(
                "Agent loop completed. Sleeping for {:?}.",
                AGENT_SLEEP_DURATION
            );
            sleep(AGENT_SLEEP_DURATION).await;
        }
    }
}

// ensure if the nodes just rebooted and re-enter the loop.
fn ensure_reboot_happened(status: &BottlerocketNodeStatus) -> Result<bool> {
    // criteria that make sure node just rebooted
    // 1. Check if system last reboot time is same as or around the timestamp that agent tried to reboot the system
    // 2. Check current system version if it's same as the version we wanted to update to. (latest version)

    let output = Command::new("uptime")
        .args(["-s"])
        .output()
        .context(error::GetUptime)?;

    // convert output string to naivedatetime
    let uptime = String::from_utf8_lossy(&output.stdout).to_string();
    let reboot_time = NaiveDateTime::parse_from_str(&uptime.trim_end(), "%Y-%m-%d %H:%M:%S")
        .context(error::ConvertStringToDatetime { uptime })?;

    // TODO: compare uptime to the timestamp that we're issuing at custom resource
    // currently we compare it to current time, if the reboot happened in the past 24 hours, we
    // assume the system just rebooted.
    let now = chrono::offset::Utc::now().naive_local();
    let duration_since_reboot = now.signed_duration_since(reboot_time).num_hours();

    Ok(duration_since_reboot <= 1 && status.available_versions[0] == status.current_version)
}

fn get_server_domain() -> String {
    format!(
        "{}.{}.svc.cluster.local:{}",
        APISERVER_SERVICE_NAME, NAMESPACE, APISERVER_SERVICE_PORT
    )
}
