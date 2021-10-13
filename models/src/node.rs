use crate::constants;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use kube::{CustomResource, ResourceExt};
use schemars::JsonSchema;
pub use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tokio::time::Duration;
use tracing::{event, span, Instrument, Level};

use std::str::FromStr;
use std::sync::Arc;

#[cfg(feature = "mockall")]
use mockall::{mock, predicate::*};

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum BottlerocketNodeError {
    #[snafu(display(
        "Unable to create BottlerocketNode ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    CreateBottlerocketNode {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display(
        "Unable to update BottlerocketNode status ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UpdateBottlerocketNodeStatus {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display(
        "Unable to update BottlerocketNode spec ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UpdateBottlerocketNodeSpec {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display("Unable to create patch to send to Kubernetes API: '{}'", source))]
    CreateK8SPatch { source: serde_json::error::Error },

    #[snafu(display("Attempted to progress node state machine without achieving current desired state. Current state: '{:?}'. Desired state: '{:?}'", current_state, desired_state))]
    NodeSpecNotAchieved {
        current_state: BottlerocketNodeState,
        desired_state: BottlerocketNodeState,
    },

    #[snafu(display("BottlerocketNode object ('{}') is missing a reference to the owning Node.", brn.name()))]
    MissingOwnerReference { brn: BottlerocketNode },
}

/// BottlerocketNodeState represents a node's state in the update state machine.
#[derive(Copy, Clone, Serialize, Deserialize, Debug, Eq, PartialEq, JsonSchema)]
pub enum BottlerocketNodeState {
    /// Nodes in this state are waiting for new updates to become available. This is both the starting and terminal state
    /// in the update process.
    WaitingForUpdate,
    /// Nodes in this state have staged a new update image and used the kubernetes cordon and drain APIs to remove
    /// running pods.
    PreparedToUpdate,
    /// Nodes in this state have installed the new image and updated the partition table to mark it as the new active
    /// image.
    PerformedUpdate,
    /// Nodes in this state have rebooted after performing an update.
    RebootedToUpdate,
    /// Nodes in this state have un-cordoned the node to allow work to be scheduled, and are monitoring to ensure that
    /// the node seems healthy before marking the udpate as complete.
    MonitoringUpdate,
}

impl Default for BottlerocketNodeState {
    fn default() -> Self {
        BottlerocketNodeState::WaitingForUpdate
    }
}

// These constants define the maximum amount of time to allow a machine to transition *into* this state.
const PREPARED_TO_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(600));
const PERFORMED_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));
const REBOOTED_TO_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(600));
const MONITORING_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(300));
const WAITING_FOR_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));

impl BottlerocketNodeState {
    /// Returns the total time that a node can spend transitioning *from* the given state to the next state in the process.
    pub fn timeout_time(&self) -> Option<Duration> {
        match self {
            &Self::WaitingForUpdate => PREPARED_TO_UPDATE_TIMEOUT,
            &Self::PreparedToUpdate => PERFORMED_UPDATE_TIMEOUT,
            &Self::PerformedUpdate => REBOOTED_TO_UPDATE_TIMEOUT,
            &Self::RebootedToUpdate => MONITORING_UPDATE_TIMEOUT,
            &Self::MonitoringUpdate => WAITING_FOR_UPDATE_TIMEOUT,
        }
    }
}

// We can't use these consts inside macros, but we do provide constants for use in generating kubernetes objects.
pub const K8S_NODE_KIND: &str = "BottlerocketNode";
pub const K8S_NODE_PLURAL: &str = "bottlerocketnodes";
pub const K8S_NODE_STATUS: &str = "bottlerocketnodes/status";
pub const K8S_NODE_SHORTNAME: &str = "brn";

/// The `BottlerocketNodeSpec` can be used to drive a node through the update state machine. A node
/// linearly drives towards the desired state. The brupop controller updates the spec to specify a node's desired state,
/// and the host agent drives state changes forward and updates the `BottlerocketNodeStatus`.
#[derive(
    Clone, CustomResource, Serialize, Deserialize, Debug, Default, Eq, PartialEq, JsonSchema,
)]
#[kube(
    derive = "Default",
    derive = "PartialEq",
    group = "brupop.bottlerocket.aws",
    kind = "BottlerocketNode",
    namespaced,
    plural = "bottlerocketnodes",
    shortname = "brn",
    singular = "bottlerocketnode",
    status = "BottlerocketNodeStatus",
    version = "v1",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.current_state"}"#,
    printcolumn = r#"{"name":"Version", "type":"string", "jsonPath":".status.current_version"}"#,
    printcolumn = r#"{"name":"Target State", "type":"string", "jsonPath":".spec.state"}"#,
    printcolumn = r#"{"name":"Target Version", "type":"string", "jsonPath":".spec.version"}"#
)]
pub struct BottlerocketNodeSpec {
    /// Records the desired state of the `BottlerocketNode`
    pub state: BottlerocketNodeState,
    /// The time at which the most recent state was set as the desired state.
    state_transition_timestamp: Option<String>,
    /// The desired update version, if any.
    version: Option<String>,
}

impl BottlerocketNode {
    /// Creates a `BottlerocketNodeSelector` from this `BottlerocketNode`.
    pub fn selector(&self) -> Result<BottlerocketNodeSelector, BottlerocketNodeError> {
        BottlerocketNodeSelector::from_bottlerocket_node(&self)
    }

    /// Constructs a `BottlerocketNodeSpec` to assign to a `BottlerocketNode` resource, assuming the current
    /// spec has been successfully achieved.
    pub fn determine_next_spec(&self) -> Result<BottlerocketNodeSpec, BottlerocketNodeError> {
        if let Some(node_status) = self.status.as_ref() {
            if node_status.current_state != self.spec.state {
                Err(BottlerocketNodeError::NodeSpecNotAchieved {
                    current_state: node_status.current_state,
                    desired_state: self.spec.state,
                })
            } else {
                match self.spec.state {
                    BottlerocketNodeState::WaitingForUpdate => {
                        // If there's a newer version available, then begin updating to that version.
                        let mut available_versions = node_status.available_versions();
                        available_versions.sort();

                        if let Some(latest_available) = available_versions.last() {
                            event!(
                                Level::TRACE,
                                ?latest_available,
                                "Found newest available version."
                            );
                            if latest_available > &node_status.current_version() {
                                event!(
                                    Level::TRACE,
                                    ?latest_available,
                                    "Latest version is newer than current version."
                                );
                                Ok(BottlerocketNodeSpec::new_starting_now(
                                    BottlerocketNodeState::PreparedToUpdate,
                                    Some(latest_available.clone()),
                                ))
                            } else {
                                Ok(BottlerocketNodeSpec::default())
                            }
                        } else {
                            Ok(BottlerocketNodeSpec::default())
                        }
                    }
                    BottlerocketNodeState::PreparedToUpdate => {
                        // Desired version stays the same, just push to the new state.
                        Ok(BottlerocketNodeSpec::new_starting_now(
                            BottlerocketNodeState::PerformedUpdate,
                            self.spec.version(),
                        ))
                    }
                    BottlerocketNodeState::PerformedUpdate => {
                        // Desired version stays the same, just push to the new state.
                        Ok(BottlerocketNodeSpec::new_starting_now(
                            BottlerocketNodeState::RebootedToUpdate,
                            self.spec.version(),
                        ))
                    }
                    BottlerocketNodeState::RebootedToUpdate => {
                        // Desired version stays the same, just push to the new state.
                        Ok(BottlerocketNodeSpec::new_starting_now(
                            BottlerocketNodeState::MonitoringUpdate,
                            self.spec.version(),
                        ))
                    }
                    BottlerocketNodeState::MonitoringUpdate => {
                        // We're ready to wait for a new update.
                        Ok(BottlerocketNodeSpec::default())
                    }
                }
            }
        } else {
            // If there is no status set, then we have no instructions for the node.
            Ok(BottlerocketNodeSpec::default())
        }
    }
}

impl BottlerocketNodeSpec {
    pub fn new(
        state: BottlerocketNodeState,
        state_transition_timestamp: Option<DateTime<Utc>>,
        version: Option<Version>,
    ) -> Self {
        let state_transition_timestamp = state_transition_timestamp.map(|ts| ts.to_rfc3339());
        let version = version.map(|v| v.to_string());
        BottlerocketNodeSpec {
            state,
            state_transition_timestamp,
            version,
        }
    }

    pub fn new_starting_now(state: BottlerocketNodeState, version: Option<Version>) -> Self {
        Self::new(state, Some(Utc::now()), version)
    }

    /// JsonSchema cannot appropriately handle DateTime objects. This accessor returns the transition timestamp
    /// as a DateTime.
    pub fn state_timestamp(&self) -> Option<DateTime<Utc>> {
        self.state_transition_timestamp.as_ref().map(|ts_str| {
            DateTime::parse_from_rfc3339(ts_str)
                .expect("state_transition_timestamp must be rfc3339 string.")
                .into()
        })
    }

    pub fn version(&self) -> Option<Version> {
        // TODO(seankell) If a user creates their own BRN object with an invalid version, this will panic.
        self.version.as_ref().map(|v| Version::from_str(v).unwrap())
    }
}

/// `BottlerocketNodeStatus` surfaces the current state of a bottlerocket node. The status is updated by the host agent,
/// while the spec is updated by the brupop controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq, JsonSchema)]
pub struct BottlerocketNodeStatus {
    current_version: String,
    available_versions: Vec<String>,
    pub current_state: BottlerocketNodeState,
}

impl BottlerocketNodeStatus {
    pub fn new(
        current_version: Version,
        available_versions: Vec<Version>,
        current_state: BottlerocketNodeState,
    ) -> Self {
        BottlerocketNodeStatus {
            current_version: current_version.to_string(),
            available_versions: available_versions.iter().map(|v| v.to_string()).collect(),
            current_state,
        }
    }

    pub fn current_version(&self) -> Version {
        // TODO(seankell) If a user creates their own BRN object with an invalid version, this will panic.
        Version::from_str(&self.current_version).unwrap()
    }

    pub fn available_versions(&self) -> Vec<Version> {
        // TODO(seankell) If a user creates their own BRN object with an invalid version, this will panic.
        self.available_versions
            .iter()
            .map(|v| Version::from_str(v).unwrap())
            .collect()
    }
}

/// Indicates the specific k8s node that BottlerocketNode object is associated with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottlerocketNodeSelector {
    pub node_name: String,
    pub node_uid: String,
}

impl BottlerocketNodeSelector {
    pub fn from_bottlerocket_node(brn: &BottlerocketNode) -> Result<Self, BottlerocketNodeError> {
        let node_owner = brn
            .metadata
            .owner_references
            .as_ref()
            .ok_or(BottlerocketNodeError::MissingOwnerReference { brn: brn.clone() })?
            .first()
            .ok_or(BottlerocketNodeError::MissingOwnerReference { brn: brn.clone() })?;

        Ok(BottlerocketNodeSelector {
            node_name: node_owner.name.clone(),
            node_uid: node_owner.uid.clone(),
        })
    }

    pub fn brn_resource_name(&self) -> String {
        format!("brn-{}", self.node_name)
    }
}

pub fn node_resource_name(node_selector: &BottlerocketNodeSelector) -> String {
    format!("brn-{}", node_selector.node_name)
}

#[async_trait]
/// A trait providing an interface to interact with BottlerocketNode objects. This is provided as a trait
/// in order to allow mocks to be used for testing purposes.
pub trait BottlerocketNodeClient: Clone + Sized + Send + Sync {
    /// Create a BottlerocketNode object for the specified node.
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError>;
    /// Update the `.status` of a BottlerocketNode object. Because the single daemon running on each node
    /// uniquely owns its brn object, we allow wholesale overwrites rather than patching.
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError>;
    /// Update the `.spec` of a BottlerocketNode object.
    // TODO: Does this need to provide helpers for Patching semantics?
    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError>;
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the status of a BottlerocketNode.
struct BottlerocketNodeStatusPatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    status: BottlerocketNodeStatus,
}

impl Default for BottlerocketNodeStatusPatch {
    fn default() -> Self {
        BottlerocketNodeStatusPatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            status: BottlerocketNodeStatus::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the spec of a BottlerocketNode.
struct BottlerocketNodeSpecPatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    spec: BottlerocketNodeSpec,
}

impl Default for BottlerocketNodeSpecPatch {
    fn default() -> Self {
        BottlerocketNodeSpecPatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            spec: BottlerocketNodeSpec::default(),
        }
    }
}

#[cfg(feature = "mockall")]
mock! {
    /// A Mock BottlerocketNodeClient for use in tests.
    pub BottlerocketNodeClient {}
    #[async_trait]
    impl BottlerocketNodeClient for BottlerocketNodeClient {
        async fn create_node(
            &self,
            selector: &BottlerocketNodeSelector,
        ) -> Result<BottlerocketNode, BottlerocketNodeError>;
        async fn update_node_status(
            &self,
            selector: &BottlerocketNodeSelector,
            status: &BottlerocketNodeStatus,
        ) -> Result<(), BottlerocketNodeError>;
        async fn update_node_spec(
            &self,
            selector: &BottlerocketNodeSelector,
            spec: &BottlerocketNodeSpec,
        ) -> Result<(), BottlerocketNodeError>;
    }

    impl Clone for BottlerocketNodeClient {
        fn clone(&self) -> Self;
    }
}

#[async_trait]
impl<T> BottlerocketNodeClient for Arc<T>
where
    T: BottlerocketNodeClient,
{
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError> {
        (**self).create_node(selector).await
    }
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError> {
        (**self).update_node_status(selector, status).await
    }

    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError> {
        (**self).update_node_spec(selector, spec).await
    }
}

#[derive(Clone)]
/// Concrete implementation of the `BottlerocketNodeClient` trait. This implementation will almost
/// certainly be used in any case that isn't a unit test.
pub struct K8SBottlerocketNodeClient {
    k8s_client: kube::client::Client,
}

impl K8SBottlerocketNodeClient {
    pub fn new(k8s_client: kube::client::Client) -> Self {
        K8SBottlerocketNodeClient { k8s_client }
    }
}

#[async_trait]
impl BottlerocketNodeClient for K8SBottlerocketNodeClient {
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError> {
        let create_span = span!(
            Level::INFO,
            "create_node",
            node_name = %selector.node_name,
            node_uid = %selector.node_uid,
        );

        // Use an asynchronous closure to avoid tracing across an await bound.
        async move {
            let br_node = BottlerocketNode {
                metadata: ObjectMeta {
                    name: Some(node_resource_name(&selector)),
                    owner_references: Some(vec![OwnerReference {
                        api_version: "v1".to_string(),
                        kind: "Node".to_string(),
                        name: selector.node_name.clone(),
                        uid: selector.node_uid.clone(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
                spec: BottlerocketNodeSpec::default(),
                ..Default::default()
            };

            event!(
                Level::INFO,
                "Sending BottlerocketNode create request to kubernetes cluster."
            );
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE)
                .create(&PostParams::default(), &br_node)
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(CreateBottlerocketNode {
                    selector: selector.clone(),
                })?;
            event!(
                Level::INFO,
                "BottlerocketNode create request completed successfully."
            );

            Ok(br_node)
        }
        .instrument(create_span)
        .await
    }

    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError> {
        let br_node_status_patch = BottlerocketNodeStatusPatch {
            status: status.clone(),
            ..Default::default()
        };
        let update_span = span!(
            Level::INFO,
            "update_node_status",
            node_name = %selector.node_name,
            node_uid = %selector.node_uid,
        );

        // Use an asynchronous closure to avoid tracing across an await bound.
        async move {
            let br_node_status_patch =
                serde_json::to_value(br_node_status_patch).context(CreateK8SPatch)?;

            let api: Api<BottlerocketNode> =
                Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

            event!(
                Level::INFO,
                "Sending BottlerocketNode patch request to kubernetes cluster.",
            );
            api.patch_status(
                &selector.brn_resource_name(),
                &PatchParams::default(),
                &Patch::Merge(&br_node_status_patch),
            )
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(UpdateBottlerocketNodeStatus {
                selector: selector.clone(),
            })?;
            event!(
                Level::INFO,
                "BottlerocketNode patch request completed successfully."
            );

            Ok(())
        }
        .instrument(update_span)
        .await
    }

    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError> {
        let br_node_spec_patch = BottlerocketNodeSpecPatch {
            spec: spec.clone(),
            ..Default::default()
        };
        let br_node_spec_patch =
            serde_json::to_value(br_node_spec_patch).context(CreateK8SPatch)?;

        let api: Api<BottlerocketNode> =
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

        api.patch(
            &selector.brn_resource_name(),
            &PatchParams::default(),
            &Patch::Merge(&br_node_spec_patch),
        )
        .await
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
        .context(UpdateBottlerocketNodeSpec {
            selector: selector.clone(),
        })?;
        Ok(())
    }
}
