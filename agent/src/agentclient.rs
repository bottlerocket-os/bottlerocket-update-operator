use k8s_openapi::api::core::v1::Node;
use kube::Api;
use snafu::{OptionExt, ResultExt};
use tokio::time::{sleep, Duration};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use kube::runtime::reflector::Store;

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

// The reflector uses exponential backoff.
// These values configure how long to delay between tries.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(10);
const NUM_RETRIES: usize = 5;

const AGENT_SLEEP_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct BrupopAgent<T: APIServerClient> {
    k8s_client: kube::client::Client,
    apiserver_client: T,
    brn_reader: Store<BottlerocketNode>,
    node_reader: Store<Node>,
    associated_node_name: String,
    associated_bottlerocketnode_name: String,
}

impl<T: APIServerClient> BrupopAgent<T> {
    pub fn new(
        k8s_client: kube::client::Client,
        apiserver_client: T,
        brn_reader: Store<BottlerocketNode>,
        node_reader: Store<Node>,
        associated_node_name: String,
        associated_bottlerocketnode_name: String,
    ) -> Self {
        BrupopAgent {
            k8s_client,
            apiserver_client,
            brn_reader,
            node_reader,
            associated_node_name,
            associated_bottlerocketnode_name,
        }
    }

    pub async fn check_node_custom_resource_exists(&mut self) -> Result<bool> {
        // try to check if node custom resource exist in the store first. If it's not present in the
        // store(either node custom resource doesn't exist or store data delays), make the API call for second round check.

        let associated_bottlerocketnode = self.brn_reader.state().clone();
        if associated_bottlerocketnode.len() != 0 {
            Ok(true)
        } else {
            let bottlerocket_nodes: Api<BottlerocketNode> =
                Api::namespaced(self.k8s_client.clone(), NAMESPACE);

            // handle the special case which custom resource does exist but communication with the k8s API fails for other errors.
            if let Err(e) = bottlerocket_nodes
                .get(&self.associated_bottlerocketnode_name.clone())
                .await
            {
                match e {
                    // 404 not found response error is OK for this use, which means custom resource doesn't exist
                    kube::Error::Api(error_response) => {
                        if error_response.code == 404 {
                            return Ok(false);
                        } else {
                            return error::FetchBottlerocketNodeErrorCode {
                                code: error_response.code,
                            }
                            .fail();
                        }
                    }
                    // Any other type of errors can not present that custom resource doesn't exist, need return error
                    _ => {
                        // TODO: Err will cause pod to be terminated and restarted, so it worths to add re-enter agent loop functionality to avoid unexpected pod termination.
                        return Err(e).context(error::UnableFetchBottlerocketNode {
                            node_name: &self.associated_bottlerocketnode_name.clone(),
                        });
                    }
                }
            }
            Ok(true)
        }
    }

    pub async fn check_custom_resource_status_exists(&self) -> Result<bool> {
        Ok(self.fetch_custom_resource().await?.status.is_some())
    }

    async fn get_node_selector(&self) -> Result<BottlerocketNodeSelector> {
        Retry::spawn(retry_strategy(), || async {
            // Agent specifies node reflector only watch and cache the node that agent pod currently lives on,
            // so vector of nodes_reader only have one object. Therefore, get_node_selector uses index 0 to extract node object.
            let node = self.node_reader.state().clone();
            if node.len() != 0 {
                let associated_node_uid = node[0]
                    .metadata
                    .uid
                    .as_ref()
                    .context(error::MissingNodeUid)?;
                return Ok(BottlerocketNodeSelector {
                    node_name: self.associated_node_name.clone(),
                    node_uid: associated_node_uid.to_string(),
                });
            }

            // reflector store is currently unavailable, bail out
            // TODO: Err will cause pod to be terminated and restarted, so it worths to add re-enter agent loop functionality to avoid unexpected pod termination.
            Err(error::Error::ReflectorUnavailable {
                object: "Node".to_string(),
            })
        })
        .await
    }

    // fetch associated bottlerocketnode (custom resource) and help to get node `metadata`, `spec`, and  `status`.
    async fn fetch_custom_resource(&self) -> Result<BottlerocketNode> {
        // get associated bottlerocketnode (custom resource) name, and we can use this
        // bottlerocketnode name to fetch targeted bottlerocketnode (custom resource) and get node `metadata`, `spec`, and  `status`.

        Retry::spawn(retry_strategy(), || async {
            // Agent specifies node reflector only watch and cache the bottlerocketnode which is associated with the node that agent pod currently lives on,
            // so vector of brn_reader only have one object. Therefore, fetch_custom_resource uses index 0 to extract bottlerocketnode object.
            let associated_bottlerocketnode = self.brn_reader.state().clone();
            if associated_bottlerocketnode.len() != 0 {
                return Ok(associated_bottlerocketnode[0].clone());
            }

            // reflector store is currently unavailable, bail out
            // TODO: Err will cause pod to be terminated and restarted, so it worths to add re-enter agent loop functionality to avoid unexpected pod termination.
            Err(error::Error::ReflectorUnavailable {
                object: "BottlerocketNode".to_string(),
            })
        })
        .await
    }

    /// Gather metadata about the system, node status provided by spec.
    async fn gather_system_metadata(
        &self,
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
        &self,
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
    pub async fn initialize_metadata_custom_resource(&self) -> Result<()> {
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
            // Create a bottlerocketnode (custom resource) if associated bottlerocketnode does not exist
            if !self.check_node_custom_resource_exists().await? {
                self.create_metadata_custom_resource().await?;
            }

            // Initialize bottlerocketnode (custom resource) `status` if associated bottlerocketnode does not have `status`
            if !self.check_custom_resource_status_exists().await? {
                self.initialize_metadata_custom_resource().await?;
            }

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
                        log::info!("Monitoring node's healthy condition");
                        // TODO: we need add some criterias here by which we decide to transition
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

fn retry_strategy() -> impl Iterator<Item = Duration> {
    ExponentialBackoff::from_millis(RETRY_BASE_DELAY.as_millis() as u64)
        .max_delay(RETRY_MAX_DELAY)
        .map(jitter)
        .take(NUM_RETRIES)
}
