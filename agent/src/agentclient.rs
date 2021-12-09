use k8s_openapi::api::core::v1::Node;
use kube::Api;
use snafu::{OptionExt, ResultExt};
use std::env;
use tokio::time::{sleep, Duration};

use crate::{
    apiclient::{boot_update, get_chosen_update, get_os_info, prepare, update},
    error::{self, Result},
};
use apiserver::{
    client::APIServerClient,
    {CreateBottlerocketNodeRequest, UpdateBottlerocketNodeRequest},
};
use models::{
    constants::NAMESPACE,
    node::{
        BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeSpec, BottlerocketNodeState,
        BottlerocketNodeStatus,
    },
};

const AGENT_SLEEP_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct BrupopAgent<T: APIServerClient> {
    k8s_client: kube::client::Client,
    node_selector: Option<BottlerocketNodeSelector>,
    apiserver_client: T,
}

impl<T: APIServerClient> BrupopAgent<T> {
    pub fn new(k8s_client: kube::client::Client, apiserver_client: T) -> Self {
        BrupopAgent {
            k8s_client,
            apiserver_client,
            node_selector: None,
        }
    }

    pub async fn check_node_custom_resource_exists(&mut self) -> Result<bool> {
        let associated_bottlerocketnode_name = self.get_node_selector().await?.brn_resource_name();
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
        let os_info = get_os_info()
            .await
            .context(error::BottlerocketNodeStatusVersion)?;
        let update_version = match get_chosen_update()
            .await
            .context(error::BottlerocketNodeStatusChosenUpdate)?
        {
            Some(chosen_update) => chosen_update.version,
            // if chosen update is null which means current node already in latest version, assign current version value to it.
            _ => os_info.version_id.clone(),
        };

        Ok(BottlerocketNodeStatus::new(
            os_info.version_id.clone(),
            update_version,
            state,
        ))
    }

    /// create the custom resource associated with this node
    pub async fn create_metadata_custom_resource(&mut self) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let brn = self
            .apiserver_client
            .create_bottlerocket_node(CreateBottlerocketNodeRequest {
                node_selector: selector,
            })
            .await
            .context(error::CreateBottlerocketNodeResource)?;

        log::info!("Created brn: '{:?}'", brn);
        Ok(())
    }

    /// update the custom resource associated with this node
    async fn update_metadata_custom_resource(
        &mut self,
        current_metadata: BottlerocketNodeStatus,
    ) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let brn_update = self
            .apiserver_client
            .update_bottlerocket_node(UpdateBottlerocketNodeRequest {
                node_selector: selector,
                node_status: current_metadata,
            })
            .await
            .context(error::UpdateBottlerocketNodeResource)?;

        log::info!("Update brn status: '{:?}'", brn_update);
        Ok(())
    }

    /// initialize bottlerocketnode (custom resource) `status` when create new bottlerocketnode
    pub async fn initialize_metadata_custom_resource(&mut self) -> Result<()> {
        let update_node_status = self
            .gather_system_metadata(BottlerocketNodeState::Idle)
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
                    BottlerocketNodeState::Idle => {
                        log::info!("Ready to finish monitoring and start update process")
                    }
                    BottlerocketNodeState::StagedUpdate => {
                        log::info!("Preparing update");
                        prepare().await.context(error::UpdateActions {
                            action: "Prepare".to_string(),
                        })?;

                        // TODO: This function needs to use the k8s drain API to remove any pods from the host,
                        // and then we need to wait until the host is successfully drained before transitioning
                        // to the next state.
                    }
                    BottlerocketNodeState::PerformedUpdate => {
                        log::info!("Performing update");
                        update().await.context(error::UpdateActions {
                            action: "Perform".to_string(),
                        })?;
                    }
                    BottlerocketNodeState::RebootedIntoUpdate => {
                        log::info!("Rebooting node to complete update");

                        if running_desired_version(&bottlerocket_node_spec).await? {
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

/// Check that the currently running version is the one requested by the controller.
async fn running_desired_version(spec: &BottlerocketNodeSpec) -> Result<bool> {
    let os_info = get_os_info()
        .await
        .context(error::BottlerocketNodeStatusVersion)?;
    Ok(match spec.version() {
        Some(spec_version) => os_info.version_id == spec_version,
        None => false,
    })
}
