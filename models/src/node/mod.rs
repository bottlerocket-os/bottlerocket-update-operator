mod client;
mod error;

pub use self::client::*;
pub use self::error::Error as BottlerocketNodeError;
use self::error::{Error, Result};

use chrono::{DateTime, Utc};
use kube::CustomResource;
use lazy_static::lazy_static;
use schemars::JsonSchema;
pub use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use tokio::time::Duration;
use validator::Validate;

use std::fmt;
use std::str::FromStr;

lazy_static! {
    // Regex gathered from semver.org as the recommended semver validation regex.
    static ref SEMVER_RE: regex::Regex = regex::Regex::new(
        concat!(
            r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)",
            r"(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?",
            r"(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$"
        ))
        .expect("Invalid regex literal.");
}

#[cfg(feature = "mockall")]
pub use self::client::MockBottlerocketNodeClient;

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
    /// Returns the next state in the state machine if the current state has been reached successfully.
    pub fn on_success(&self) -> Self {
        match self {
            Self::WaitingForUpdate => Self::PreparedToUpdate,
            Self::PreparedToUpdate => Self::PerformedUpdate,
            Self::PerformedUpdate => Self::RebootedToUpdate,
            Self::RebootedToUpdate => Self::MonitoringUpdate,
            Self::MonitoringUpdate => Self::WaitingForUpdate,
        }
    }

    /// Returns the total time that a node can spend transitioning *from* the given state to the next state in the process.
    pub fn timeout_time(&self) -> Option<Duration> {
        match self {
            Self::WaitingForUpdate => PREPARED_TO_UPDATE_TIMEOUT,
            Self::PreparedToUpdate => PERFORMED_UPDATE_TIMEOUT,
            Self::PerformedUpdate => REBOOTED_TO_UPDATE_TIMEOUT,
            Self::RebootedToUpdate => MONITORING_UPDATE_TIMEOUT,
            Self::MonitoringUpdate => WAITING_FOR_UPDATE_TIMEOUT,
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
    Clone,
    CustomResource,
    Serialize,
    Deserialize,
    Debug,
    Default,
    Eq,
    PartialEq,
    JsonSchema,
    Validate,
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
    #[validate(regex = "SEMVER_RE")]
    version: Option<String>,
}

impl BottlerocketNode {
    /// Creates a `BottlerocketNodeSelector` from this `BottlerocketNode`.
    pub fn selector(&self) -> Result<BottlerocketNodeSelector> {
        BottlerocketNodeSelector::from_bottlerocket_node(self)
    }

    /// Returns whether or not a node has reached the state requested by its spec.
    pub fn has_reached_desired_state(&self) -> bool {
        self.status.as_ref().map_or(false, |node_status| {
            node_status.current_state == self.spec.state
        })
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

    /// Creates a new BottlerocketNodeSpec, using the current time as the timestamp for timeout purposes.
    pub fn new_starting_now(state: BottlerocketNodeState, version: Option<Version>) -> Self {
        Self::new(state, Some(Utc::now()), version)
    }

    /// JsonSchema cannot appropriately handle DateTime objects. This accessor returns the transition timestamp
    /// as a DateTime.
    pub fn state_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        self.state_transition_timestamp
            .as_ref()
            .map(|ts_str| {
                DateTime::parse_from_rfc3339(ts_str)
                    // Convert `DateTime<FixedOffset>` into `DateTime<Utc>`
                    .map(|ts| ts.into())
                    .context(error::TimestampFormat)
            })
            .transpose()
    }

    /// Returns the desired version for this BottlerocketNode.
    pub fn version(&self) -> Option<Version> {
        // We know this won't panic because we have a regex requirement on this attribute, which is enforced by the k8s schema.
        self.version.as_ref().map(|v| Version::from_str(v).unwrap())
    }
}

/// `BottlerocketNodeStatus` surfaces the current state of a bottlerocket node. The status is updated by the host agent,
/// while the spec is updated by the brupop controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq, JsonSchema)]
pub struct BottlerocketNodeStatus {
    #[validate(regex = "SEMVER_RE")]
    current_version: String,
    // TODO We haven't configured validations against `available_versions`, but we are planning to remove this field.
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
        // We know this won't panic because we have a regex requirement on this attribute, which is enforced by the k8s schema.
        Version::from_str(&self.current_version).unwrap()
    }

    pub fn available_versions(&self) -> Vec<Version> {
        // TODO This could panic if a custom `brn` is created with improperly specified versions; however, we are removing this
        // attribute in an impending iteration, so we won't fix it.
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
    pub fn from_bottlerocket_node(brn: &BottlerocketNode) -> Result<Self> {
        let node_owner = brn
            .metadata
            .owner_references
            .as_ref()
            .ok_or(Error::MissingOwnerReference { brn: brn.clone() })?
            .first()
            .ok_or(Error::MissingOwnerReference { brn: brn.clone() })?;

        Ok(BottlerocketNodeSelector {
            node_name: node_owner.name.clone(),
            node_uid: node_owner.uid.clone(),
        })
    }

    pub fn brn_resource_name(&self) -> String {
        format!("brn-{}", self.node_name)
    }
}

impl fmt::Display for BottlerocketNodeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.node_name, self.node_uid)
    }
}
