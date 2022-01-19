use k8s_openapi::api::core::v1::Node;
use kube::Api;
use snafu::{OptionExt, ResultExt};
use tokio::time::{sleep, Duration};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};
use tracing::{event, instrument, Level};

use kube::runtime::reflector::Store;

use crate::apiclient::{boot_update, get_chosen_update, get_os_info, prepare, update};
use apiserver::{
    client::APIServerClient,
    CordonAndDrainBottlerocketNodeRequest, UncordonBottlerocketNodeRequest,
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

/// The module-wide result type.
pub type Result<T> = std::result::Result<T, agentclient_error::Error>;

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

    #[instrument(skip(self), err)]
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
                            return agentclient_error::FetchBottlerocketNodeErrorCode {
                                code: error_response.code,
                            }
                            .fail();
                        }
                    }
                    // Any other type of errors can not present that custom resource doesn't exist, need return error
                    _ => {
                        return Err(e).context(agentclient_error::UnableFetchBottlerocketNode {
                            node_name: &self.associated_bottlerocketnode_name.clone(),
                        });
                    }
                }
            }
            Ok(true)
        }
    }

    #[instrument(skip(self), err)]
    pub async fn check_custom_resource_status_exists(&self) -> Result<bool> {
        Ok(self.fetch_custom_resource().await?.status.is_some())
    }

    #[instrument(skip(self), err)]
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
                    .context(agentclient_error::MissingNodeUid)?;
                return Ok(BottlerocketNodeSelector {
                    node_name: self.associated_node_name.clone(),
                    node_uid: associated_node_uid.to_string(),
                });
            }

            // reflector store is currently unavailable, bail out
            Err(agentclient_error::Error::ReflectorUnavailable {
                object: "Node".to_string(),
            })
        })
        .await
    }

    // fetch associated bottlerocketnode (custom resource) and help to get node `metadata`, `spec`, and  `status`.
    #[instrument(skip(self), err)]
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
            Err(agentclient_error::Error::ReflectorUnavailable {
                object: "BottlerocketNode".to_string(),
            })
        })
        .await
    }

    /// Gather metadata about the system, node status provided by spec.
    #[instrument(skip(self, state), err)]
    async fn gather_system_metadata(
        &self,
        state: BottlerocketNodeState,
    ) -> Result<BottlerocketNodeStatus> {
        let os_info = get_os_info()
            .await
            .context(agentclient_error::BottlerocketNodeStatusVersion)?;
        let update_version = match get_chosen_update()
            .await
            .context(agentclient_error::BottlerocketNodeStatusChosenUpdate)?
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
    #[instrument(skip(self), err)]
    pub async fn create_metadata_custom_resource(&mut self) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let _brn = self
            .apiserver_client
            .create_bottlerocket_node(CreateBottlerocketNodeRequest {
                node_selector: selector.clone(),
            })
            .await
            .context(agentclient_error::CreateBottlerocketNodeResource)?;
        event!(Level::INFO, brn_name = ?selector.node_name, "Brn has been created.");
        Ok(())
    }

    /// update the custom resource associated with this node
    #[instrument(skip(self, current_metadata), err)]
    async fn update_metadata_custom_resource(
        &self,
        current_metadata: BottlerocketNodeStatus,
    ) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let brn_update = self
            .apiserver_client
            .update_bottlerocket_node(UpdateBottlerocketNodeRequest {
                node_selector: selector.clone(),
                node_status: current_metadata,
            })
            .await
            .context(agentclient_error::UpdateBottlerocketNodeResource)?;
        event!(Level::INFO, brn_name = ?selector.node_name, brn_status = ?brn_update, "Brn status has been updated.");
        Ok(())
    }

    /// initialize bottlerocketnode (custom resource) `status` when create new bottlerocketnode
    #[instrument(skip(self), err)]
    pub async fn initialize_metadata_custom_resource(&self) -> Result<()> {
        let update_node_status = self
            .gather_system_metadata(BottlerocketNodeState::Idle)
            .await?;

        self.update_metadata_custom_resource(update_node_status)
            .await?;
        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn cordon_and_drain(&self) -> Result<()> {
        let selector = self.get_node_selector().await?;

        self.apiserver_client
            .cordon_and_drain_node(CordonAndDrainBottlerocketNodeRequest {
                node_selector: selector,
            })
            .await
            .context(agentclient_error::CordonAndDrainNode)?;

        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn uncordon(&self) -> Result<()> {
        let selector = self.get_node_selector().await?;

        self.apiserver_client
            .uncordon_node(UncordonBottlerocketNodeRequest {
                node_selector: selector,
            })
            .await
            .context(agentclient_error::UncordonNode)?;

        Ok(())
    }

    #[instrument(skip(self), err)]
    pub async fn run(&mut self) -> Result<()> {
        // A running agent has two responsibilities:
        // - Gather metadata about the system and update the custom resource associated with this node
        // - Determine if the spec on the system's custom resource demands the node take action. If so, begin taking that action.

        loop {
            // Create a bottlerocketnode (custom resource) if associated bottlerocketnode does not exist
            if self.check_node_custom_resource_exists().await.is_err() {
                // Errors checking if brn exists are ignored (and also logged by `check_node_custom_resource_exists()`).
                event!(Level::WARN, "An error occurred when checking if BottlerocketNode exists. Restarting event loop");
                sleep(AGENT_SLEEP_DURATION).await;
                continue;
            } else {
                if !self.check_node_custom_resource_exists().await? {
                    if self.create_metadata_custom_resource().await.is_err() {
                        // Errors creating brn are ignored (and also logged by `create_metadata_custom_resource()`).
                        event!(Level::WARN, "An error occurred when creating BottlerocketNode. Restarting event loop");
                        sleep(AGENT_SLEEP_DURATION).await;
                        continue;
                    }
                }
            }

            // Initialize bottlerocketnode (custom resource) `status` if associated bottlerocketnode does not have `status`
            if self.check_custom_resource_status_exists().await.is_err() {
                // Errors checking if brn status exists are ignored (and also logged by `check_custom_resource_status_exists()`).
                event!(Level::WARN, "An error occurred when checking if BottlerocketNode status exists. Restarting event loop");
                sleep(AGENT_SLEEP_DURATION).await;
                continue;
            } else {
                if !self.check_custom_resource_status_exists().await? {
                    if self.initialize_metadata_custom_resource().await.is_err() {
                        // Errors initializing brn are ignored (and also logged by `initialize_metadata_custom_resource()`).
                        event!(Level::WARN, "An error occurred when initializing BottlerocketNode. Restarting event loop");
                        sleep(AGENT_SLEEP_DURATION).await;
                        continue;
                    }
                }
            }

            // Requests metadata 'status' and 'spec' for the current BottlerocketNode
            let bottlerocket_node = match self.fetch_custom_resource().await {
                Ok(bottlerocket_node) => bottlerocket_node,
                Err(_) => {
                    // Errors fetching brn are ignored (and also logged by `fetch_custom_resource()`).
                    event!(
                        Level::WARN,
                        "An error occurred when fetching BottlerocketNode. Restarting event loop"
                    );
                    sleep(AGENT_SLEEP_DURATION).await;
                    continue;
                }
            };

            let bottlerocket_node_status = bottlerocket_node
                .status
                .context(agentclient_error::MissingBottlerocketNodeStatus)?;
            let bottlerocket_node_spec = bottlerocket_node.spec;

            // Determine if the spec on the system's custom resource demands the node take action. If so, begin taking that action.
            if bottlerocket_node_spec.state != bottlerocket_node_status.current_state {
                event!(
                    Level::INFO,
                    brn_name = ?bottlerocket_node.metadata.name,
                    action = ?bottlerocket_node_spec.state,
                    "Detected drift between spec state and current state. Requesting node to take action"
                );

                match bottlerocket_node_spec.state {
                    BottlerocketNodeState::Idle => {
                        event!(
                            Level::INFO,
                            "Ready to finish monitoring and start update process"
                        );
                    }
                    BottlerocketNodeState::StagedUpdate => {
                        event!(Level::INFO, "Preparing update");
                        prepare().await.context(agentclient_error::UpdateActions {
                            action: "Prepare".to_string(),
                        })?;

                        self.cordon_and_drain().await?;
                    }
                    BottlerocketNodeState::PerformedUpdate => {
                        event!(Level::INFO, "Performing update");
                        update().await.context(agentclient_error::UpdateActions {
                            action: "Perform".to_string(),
                        })?;
                    }
                    BottlerocketNodeState::RebootedIntoUpdate => {
                        event!(Level::INFO, "Rebooting node to complete update");

                        if running_desired_version(&bottlerocket_node_spec).await? {
                            // previous execution `reboot` exited loop and did not update the custom resource
                            // associated with this node. When re-enter loop and finished reboot,
                            // try to update the custom resource.
                            let update_node_status = match self
                                .gather_system_metadata(bottlerocket_node_spec.state.clone())
                                .await
                            {
                                Ok(update_node_status) => update_node_status,
                                Err(_) => {
                                    // Errors gathering system metadata are ignored (and also logged by `gather_system_metadata()`).
                                    event!(Level::WARN, "An error occurred when gathering system metadata. Restarting event loop");
                                    sleep(AGENT_SLEEP_DURATION).await;
                                    continue;
                                }
                            };
                            if self
                                .update_metadata_custom_resource(update_node_status)
                                .await
                                .is_err()
                            {
                                // Errors updating BottlerocketNode are ignored (and also logged by `update_metadata_custom_resource()`).
                                event!(Level::WARN, "An error occurred when updating BottlerocketNode. Restarting event loop");
                                sleep(AGENT_SLEEP_DURATION).await;
                                continue;
                            };
                        } else {
                            boot_update()
                                .await
                                .context(agentclient_error::UpdateActions {
                                    action: "Reboot".to_string(),
                                })?;
                        }
                    }
                    BottlerocketNodeState::MonitoringUpdate => {
                        event!(Level::INFO, "Monitoring node's healthy condition");
                        self.uncordon().await?;
                        // TODO: we need add some criterias here by which we decide to transition
                        // from MonitoringUpdate to WaitingForUpdate.
                    }
                }
            } else {
                event!(Level::INFO, "Did not detect action demand.");
            }

            // update the custom resource associated with this node
            let updated_node_status = match self
                .gather_system_metadata(bottlerocket_node_spec.state.clone())
                .await
            {
                Ok(updated_node_status) => updated_node_status,
                Err(_) => {
                    // Errors gathering system metadata are ignored (and also logged by `gather_system_metadata()`).
                    event!(
                        Level::WARN,
                        "An error occurred when gathering system metadata. Restarting event loop"
                    );
                    sleep(AGENT_SLEEP_DURATION).await;
                    continue;
                }
            };
            if updated_node_status != bottlerocket_node_status {
                if self
                    .update_metadata_custom_resource(updated_node_status)
                    .await
                    .is_err()
                {
                    // Errors updating BottlerocketNode are ignored (and also logged by `update_metadata_custom_resource()`).
                    event!(
                        Level::WARN,
                        "An error occurred when updating BottlerocketNode. Restarting event loop"
                    );
                    sleep(AGENT_SLEEP_DURATION).await;
                    continue;
                };
            }

            event!(Level::DEBUG, "Agent loop completed. Sleeping.....");
            sleep(AGENT_SLEEP_DURATION).await;
        }
    }
}

/// Check that the currently running version is the one requested by the controller.
async fn running_desired_version(spec: &BottlerocketNodeSpec) -> Result<bool> {
    let os_info = get_os_info()
        .await
        .context(agentclient_error::BottlerocketNodeStatusVersion)?;
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

pub mod agentclient_error {
    use crate::apiclient::apiclient_error;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum Error {
        #[snafu(display("Unable to gather system version metadata: {}", source))]
        BottlerocketNodeStatusVersion { source: apiclient_error::Error },

        #[snafu(display("Unable to gather system chosen update metadata: '{}'", source))]
        BottlerocketNodeStatusChosenUpdate { source: apiclient_error::Error },

        #[snafu(display("Unable to drain and cordon this node: '{}'", source))]
        CordonAndDrainNode {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "Unable to create the custom resource associated with this node: '{}'",
            source
        ))]
        CreateBottlerocketNodeResource {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "ErrorResponse code '{}' when sending to fetch Bottlerocket Node",
            code
        ))]
        FetchBottlerocketNodeErrorCode { code: u16 },

        #[snafu(display(
            "Unable to get Bottlerocket node 'status' because of missing 'status' value"
        ))]
        MissingBottlerocketNodeStatus,

        #[snafu(display("Unable to get Node uid because of missing Node `uid` value"))]
        MissingNodeUid {},

        #[snafu(display(
            "Unable to fetch {} store: Store unavailable: retries exhausted",
            object
        ))]
        ReflectorUnavailable { object: String },

        #[snafu(display("Unable to uncordon this node: '{}'", source))]
        UncordonNode {
            source: apiserver::client::ClientError,
        },

        #[snafu(display("Unable to take action '{}': '{}'", action, source))]
        UpdateActions {
            action: String,
            source: apiclient_error::Error,
        },

        #[snafu(display(
            "Unable to update the custom resource associated with this node: '{}'",
            source
        ))]
        UpdateBottlerocketNodeResource {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "Error {} when sending to fetch Bottlerocket Node {}",
            source,
            node_name
        ))]
        UnableFetchBottlerocketNode {
            node_name: String,
            source: kube::Error,
        },
    }
}
