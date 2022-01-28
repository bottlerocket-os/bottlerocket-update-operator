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
    CordonAndDrainBottlerocketShadowRequest, UncordonBottlerocketShadowRequest,
    {CreateBottlerocketShadowRequest, UpdateBottlerocketShadowRequest},
};
use models::{
    constants::NAMESPACE,
    node::{
        BottlerocketShadow, BottlerocketShadowSelector, BottlerocketShadowSpec,
        BottlerocketShadowState, BottlerocketShadowStatus,
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
    brs_reader: Store<BottlerocketShadow>,
    node_reader: Store<Node>,
    associated_node_name: String,
    associated_bottlerocketshadow_name: String,
}

impl<T: APIServerClient> BrupopAgent<T> {
    pub fn new(
        k8s_client: kube::client::Client,
        apiserver_client: T,
        brs_reader: Store<BottlerocketShadow>,
        node_reader: Store<Node>,
        associated_node_name: String,
        associated_bottlerocketshadow_name: String,
    ) -> Self {
        BrupopAgent {
            k8s_client,
            apiserver_client,
            brs_reader,
            node_reader,
            associated_node_name,
            associated_bottlerocketshadow_name,
        }
    }

    #[instrument(skip(self), err)]
    pub async fn check_node_custom_resource_exists(&mut self) -> Result<bool> {
        // try to check if node custom resource exist in the store first. If it's not present in the
        // store(either node custom resource doesn't exist or store data delays), make the API call for second round check.

        let associated_bottlerocketshadow = self.brs_reader.state().clone();
        if associated_bottlerocketshadow.len() != 0 {
            Ok(true)
        } else {
            let bottlerocket_shadows: Api<BottlerocketShadow> =
                Api::namespaced(self.k8s_client.clone(), NAMESPACE);

            // handle the special case which custom resource does exist but communication with the k8s API fails for other errors.
            if let Err(e) = bottlerocket_shadows
                .get(&self.associated_bottlerocketshadow_name.clone())
                .await
            {
                match e {
                    // 404 not found response error is OK for this use, which means custom resource doesn't exist
                    kube::Error::Api(error_response) => {
                        if error_response.code == 404 {
                            return Ok(false);
                        } else {
                            return agentclient_error::FetchBottlerocketShadowErrorCode {
                                code: error_response.code,
                            }
                            .fail();
                        }
                    }
                    // Any other type of errors can not present that custom resource doesn't exist, need return error
                    _ => {
                        return Err(e).context(agentclient_error::UnableFetchBottlerocketShadow {
                            node_name: &self.associated_bottlerocketshadow_name.clone(),
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
    async fn get_node_selector(&self) -> Result<BottlerocketShadowSelector> {
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
                return Ok(BottlerocketShadowSelector {
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

    // fetch associated BottlerocketShadow (custom resource) and help to get node `metadata`, `spec`, and  `status`.
    #[instrument(skip(self), err)]
    async fn fetch_custom_resource(&self) -> Result<BottlerocketShadow> {
        // get associated BottlerocketShadow (custom resource) name, and we can use this
        // BottlerocketShadow name to fetch targeted BottlerocketShadow (custom resource) and get node `metadata`, `spec`, and  `status`.

        Retry::spawn(retry_strategy(), || async {
            // Agent specifies node reflector only watch and cache the BottlerocketShadow which is associated with the node that agent pod currently lives on,
            // so vector of brs_reader only have one object. Therefore, fetch_custom_resource uses index 0 to extract BottlerocketShadow object.
            let associated_bottlerocketshadow = self.brs_reader.state().clone();
            if associated_bottlerocketshadow.len() != 0 {
                return Ok(associated_bottlerocketshadow[0].clone());
            }

            // reflector store is currently unavailable, bail out
            Err(agentclient_error::Error::ReflectorUnavailable {
                object: "BottlerocketShadow".to_string(),
            })
        })
        .await
    }

    /// Gather metadata about the system, node status provided by spec.
    #[instrument(skip(self, state), err)]
    async fn gather_system_metadata(
        &self,
        state: BottlerocketShadowState,
    ) -> Result<BottlerocketShadowStatus> {
        let os_info = get_os_info()
            .await
            .context(agentclient_error::BottlerocketShadowStatusVersion)?;
        let update_version = match get_chosen_update()
            .await
            .context(agentclient_error::BottlerocketShadowStatusChosenUpdate)?
        {
            Some(chosen_update) => chosen_update.version,
            // if chosen update is null which means current node already in latest version, assign current version value to it.
            _ => os_info.version_id.clone(),
        };

        Ok(BottlerocketShadowStatus::new(
            os_info.version_id.clone(),
            update_version,
            state,
        ))
    }

    /// create the custom resource associated with this node
    #[instrument(skip(self), err)]
    pub async fn create_metadata_custom_resource(&mut self) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let _brs = self
            .apiserver_client
            .create_bottlerocket_shadow(CreateBottlerocketShadowRequest {
                node_selector: selector.clone(),
            })
            .await
            .context(agentclient_error::CreateBottlerocketShadowResource)?;
        event!(Level::INFO, brs_name = ?selector.node_name, "Brs has been created.");
        Ok(())
    }

    /// update the custom resource associated with this node
    #[instrument(skip(self, current_metadata), err)]
    async fn update_metadata_custom_resource(
        &self,
        current_metadata: BottlerocketShadowStatus,
    ) -> Result<()> {
        let selector = self.get_node_selector().await?;
        let brs_update = self
            .apiserver_client
            .update_bottlerocket_shadow(UpdateBottlerocketShadowRequest {
                node_selector: selector.clone(),
                node_status: current_metadata,
            })
            .await
            .context(agentclient_error::UpdateBottlerocketShadowResource)?;
        event!(Level::INFO, brs_name = ?selector.node_name, brs_status = ?brs_update, "Brs status has been updated.");
        Ok(())
    }

    /// initialize BottlerocketShadow (custom resource) `status` when create new BottlerocketShadow
    #[instrument(skip(self), err)]
    pub async fn initialize_metadata_custom_resource(&self) -> Result<()> {
        let update_node_status = self
            .gather_system_metadata(BottlerocketShadowState::Idle)
            .await?;

        self.update_metadata_custom_resource(update_node_status)
            .await?;
        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn cordon_and_drain(&self) -> Result<()> {
        let selector = self.get_node_selector().await?;

        self.apiserver_client
            .cordon_and_drain_node(CordonAndDrainBottlerocketShadowRequest {
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
            .uncordon_node(UncordonBottlerocketShadowRequest {
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
            // Create a BottlerocketShadow (custom resource) if associated BottlerocketShadow does not exist
            if self.check_node_custom_resource_exists().await.is_err() {
                // Errors checking if brs exists are ignored (and also logged by `check_node_custom_resource_exists()`).
                event!(Level::WARN, "An error occurred when checking if BottlerocketShadow exists. Restarting event loop");
                sleep(AGENT_SLEEP_DURATION).await;
                continue;
            } else {
                if !self.check_node_custom_resource_exists().await? {
                    if self.create_metadata_custom_resource().await.is_err() {
                        // Errors creating brs are ignored (and also logged by `create_metadata_custom_resource()`).
                        event!(Level::WARN, "An error occurred when creating BottlerocketShadow. Restarting event loop");
                        sleep(AGENT_SLEEP_DURATION).await;
                        continue;
                    }
                }
            }

            // Initialize BottlerocketShadow (custom resource) `status` if associated BottlerocketShadow does not have `status`
            if self.check_custom_resource_status_exists().await.is_err() {
                // Errors checking if brs status exists are ignored (and also logged by `check_custom_resource_status_exists()`).
                event!(Level::WARN, "An error occurred when checking if BottlerocketShadow status exists. Restarting event loop");
                sleep(AGENT_SLEEP_DURATION).await;
                continue;
            } else {
                if !self.check_custom_resource_status_exists().await? {
                    if self.initialize_metadata_custom_resource().await.is_err() {
                        // Errors initializing brs are ignored (and also logged by `initialize_metadata_custom_resource()`).
                        event!(Level::WARN, "An error occurred when initializing BottlerocketShadow. Restarting event loop");
                        sleep(AGENT_SLEEP_DURATION).await;
                        continue;
                    }
                }
            }

            // Requests metadata 'status' and 'spec' for the current BottlerocketShadow
            let bottlerocket_shadow = match self.fetch_custom_resource().await {
                Ok(bottlerocket_shadow) => bottlerocket_shadow,
                Err(_) => {
                    // Errors fetching brs are ignored (and also logged by `fetch_custom_resource()`).
                    event!(
                        Level::WARN,
                        "An error occurred when fetching BottlerocketShadow. Restarting event loop"
                    );
                    sleep(AGENT_SLEEP_DURATION).await;
                    continue;
                }
            };

            let bottlerocket_shadow_status = bottlerocket_shadow
                .status
                .context(agentclient_error::MissingBottlerocketShadowStatus)?;
            let bottlerocket_shadow_spec = bottlerocket_shadow.spec;

            // Determine if the spec on the system's custom resource demands the node take action. If so, begin taking that action.
            if bottlerocket_shadow_spec.state != bottlerocket_shadow_status.current_state {
                event!(
                    Level::INFO,
                    brs_name = ?bottlerocket_shadow.metadata.name,
                    action = ?bottlerocket_shadow_spec.state,
                    "Detected drift between spec state and current state. Requesting node to take action"
                );

                match bottlerocket_shadow_spec.state {
                    BottlerocketShadowState::Idle => {
                        event!(
                            Level::INFO,
                            "Ready to finish monitoring and start update process"
                        );
                    }
                    BottlerocketShadowState::StagedUpdate => {
                        event!(Level::INFO, "Preparing update");
                        prepare().await.context(agentclient_error::UpdateActions {
                            action: "Prepare".to_string(),
                        })?;

                        self.cordon_and_drain().await?;
                    }
                    BottlerocketShadowState::PerformedUpdate => {
                        event!(Level::INFO, "Performing update");
                        update().await.context(agentclient_error::UpdateActions {
                            action: "Perform".to_string(),
                        })?;
                    }
                    BottlerocketShadowState::RebootedIntoUpdate => {
                        event!(Level::INFO, "Rebooting node to complete update");

                        if running_desired_version(&bottlerocket_shadow_spec).await? {
                            // previous execution `reboot` exited loop and did not update the custom resource
                            // associated with this node. When re-enter loop and finished reboot,
                            // try to update the custom resource.
                            let update_node_status = match self
                                .gather_system_metadata(bottlerocket_shadow_spec.state.clone())
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
                                // Errors updating BottlerocketShadow are ignored (and also logged by `update_metadata_custom_resource()`).
                                event!(Level::WARN, "An error occurred when updating BottlerocketShadow. Restarting event loop");
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
                    BottlerocketShadowState::MonitoringUpdate => {
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
                .gather_system_metadata(bottlerocket_shadow_spec.state.clone())
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
            if updated_node_status != bottlerocket_shadow_status {
                if self
                    .update_metadata_custom_resource(updated_node_status)
                    .await
                    .is_err()
                {
                    // Errors updating BottlerocketShadow are ignored (and also logged by `update_metadata_custom_resource()`).
                    event!(
                        Level::WARN,
                        "An error occurred when updating BottlerocketShadow. Restarting event loop"
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
async fn running_desired_version(spec: &BottlerocketShadowSpec) -> Result<bool> {
    let os_info = get_os_info()
        .await
        .context(agentclient_error::BottlerocketShadowStatusVersion)?;
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
        BottlerocketShadowStatusVersion { source: apiclient_error::Error },

        #[snafu(display("Unable to gather system chosen update metadata: '{}'", source))]
        BottlerocketShadowStatusChosenUpdate { source: apiclient_error::Error },

        #[snafu(display("Unable to drain and cordon this node: '{}'", source))]
        CordonAndDrainNode {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "Unable to create the custom resource associated with this node: '{}'",
            source
        ))]
        CreateBottlerocketShadowResource {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "ErrorResponse code '{}' when sending to fetch Bottlerocket Node",
            code
        ))]
        FetchBottlerocketShadowErrorCode { code: u16 },

        #[snafu(display(
            "Unable to get Bottlerocket node 'status' because of missing 'status' value"
        ))]
        MissingBottlerocketShadowStatus,

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
        UpdateBottlerocketShadowResource {
            source: apiserver::client::ClientError,
        },

        #[snafu(display(
            "Error {} when sending to fetch Bottlerocket Node {}",
            source,
            node_name
        ))]
        UnableFetchBottlerocketShadow {
            node_name: String,
            source: kube::Error,
        },
    }
}
