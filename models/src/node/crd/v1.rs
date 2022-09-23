use super::BottlerocketShadowResource;
use crate::node::{error, SEMVER_RE};

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
pub use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::str::FromStr;
use tokio::time::Duration;
use validator::Validate;

/// BottlerocketShadowState represents a node's state in the update state machine.
#[derive(Copy, Clone, Serialize, Deserialize, Debug, Eq, PartialEq, JsonSchema)]
pub enum BottlerocketShadowState {
    /// Nodes in this state are waiting for new updates to become available. This is both the starting and terminal state
    /// in the update process.
    Idle,
    /// Nodes in this state have staged a new update image and used the kubernetes cordon and drain APIs to remove
    /// running pods.
    StagedUpdate,
    /// Nodes in this state have installed the new image and updated the partition table to mark it as the new active
    /// image.
    PerformedUpdate,
    /// Nodes in this state have rebooted after performing an update.
    RebootedIntoUpdate,
    /// Nodes in this state have un-cordoned the node to allow work to be scheduled, and are monitoring to ensure that
    /// the node seems healthy before marking the udpate as complete.
    MonitoringUpdate,
}

impl Default for BottlerocketShadowState {
    fn default() -> Self {
        BottlerocketShadowState::Idle
    }
}

// These constants define the maximum amount of time to allow a machine to transition *into* this state.
const STAGED_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(600));
const PERFORMED_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));
const REBOOTED_INTO_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(600));
const MONITORING_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(300));
const IDLE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));

impl BottlerocketShadowState {
    /// Returns the next state in the state machine if the current state has been reached successfully.
    pub fn on_success(&self) -> Self {
        match self {
            Self::Idle => Self::StagedUpdate,
            Self::StagedUpdate => Self::PerformedUpdate,
            Self::PerformedUpdate => Self::RebootedIntoUpdate,
            Self::RebootedIntoUpdate => Self::MonitoringUpdate,
            Self::MonitoringUpdate => Self::Idle,
        }
    }

    /// Returns the total time that a node can spend transitioning *from* the given state to the next state in the process.
    pub fn timeout_time(&self) -> Option<Duration> {
        match self {
            Self::Idle => IDLE_TIMEOUT,
            Self::StagedUpdate => STAGED_UPDATE_TIMEOUT,
            Self::PerformedUpdate => PERFORMED_UPDATE_TIMEOUT,
            Self::RebootedIntoUpdate => REBOOTED_INTO_UPDATE_TIMEOUT,
            Self::MonitoringUpdate => MONITORING_UPDATE_TIMEOUT,
        }
    }
}

/// The `BottlerocketShadowSpec` can be used to drive a node through the update state machine. A node
/// linearly drives towards the desired state. The brupop controller updates the spec to specify a node's desired state,
/// and the host agent drives state changes forward and updates the `BottlerocketShadowStatus`.
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
    kind = "BottlerocketShadow",
    namespaced,
    plural = "bottlerocketshadows",
    shortname = "brs",
    singular = "bottlerocketshadow",
    status = "BottlerocketShadowStatus",
    version = "v1",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.current_state"}"#,
    printcolumn = r#"{"name":"Version", "type":"string", "jsonPath":".status.current_version"}"#,
    printcolumn = r#"{"name":"Target State", "type":"string", "jsonPath":".spec.state"}"#,
    printcolumn = r#"{"name":"Target Version", "type":"string", "jsonPath":".spec.version"}"#
)]
pub struct BottlerocketShadowSpec {
    /// Records the desired state of the `BottlerocketShadow`
    pub state: BottlerocketShadowState,
    /// The time at which the most recent state was set as the desired state.
    state_transition_timestamp: Option<String>,
    /// The desired update version, if any.
    #[validate(regex = "SEMVER_RE")]
    version: Option<String>,
}

impl BottlerocketShadowResource for BottlerocketShadow {}

impl BottlerocketShadow {
    /// Returns whether or not a node has reached the state requested by its spec.
    pub fn has_reached_desired_state(&self) -> bool {
        self.status.as_ref().map_or(false, |node_status| {
            node_status.current_state == self.spec.state
        })
    }
}

impl BottlerocketShadowSpec {
    pub fn new(
        state: BottlerocketShadowState,
        state_transition_timestamp: Option<DateTime<Utc>>,
        version: Option<Version>,
    ) -> Self {
        let state_transition_timestamp = state_transition_timestamp.map(|ts| ts.to_rfc3339());
        let version = version.map(|v| v.to_string());
        BottlerocketShadowSpec {
            state,
            state_transition_timestamp,
            version,
        }
    }

    /// Creates a new BottlerocketShadowSpec, using the current time as the timestamp for timeout purposes.
    pub fn new_starting_now(state: BottlerocketShadowState, version: Option<Version>) -> Self {
        Self::new(state, Some(Utc::now()), version)
    }

    /// JsonSchema cannot appropriately handle DateTime objects. This accessor returns the transition timestamp
    /// as a DateTime.
    pub fn state_timestamp(&self) -> error::Result<Option<DateTime<Utc>>> {
        self.state_transition_timestamp
            .as_ref()
            .map(|ts_str| {
                DateTime::parse_from_rfc3339(ts_str)
                    // Convert `DateTime<FixedOffset>` into `DateTime<Utc>`
                    .map(|ts| ts.into())
                    .context(error::TimestampFormatSnafu)
            })
            .transpose()
    }

    /// Returns the desired version for this BottlerocketShadow.
    pub fn version(&self) -> Option<Version> {
        // We know this won't panic because we have a regex requirement on this attribute, which is enforced by the k8s schema.
        self.version.as_ref().map(|v| Version::from_str(v).unwrap())
    }
}

/// `BottlerocketShadowStatus` surfaces the current state of a bottlerocket node. The status is updated by the host agent,
/// while the spec is updated by the brupop controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq, JsonSchema)]
pub struct BottlerocketShadowStatus {
    #[validate(regex = "SEMVER_RE")]
    current_version: String,
    #[validate(regex = "SEMVER_RE")]
    target_version: String,
    pub current_state: BottlerocketShadowState,
}

impl BottlerocketShadowStatus {
    pub fn new(
        current_version: Version,
        target_version: Version,
        current_state: BottlerocketShadowState,
    ) -> Self {
        BottlerocketShadowStatus {
            current_version: current_version.to_string(),
            target_version: target_version.to_string(),
            current_state,
        }
    }

    pub fn current_version(&self) -> Version {
        // We know this won't panic because we have a regex requirement on this attribute, which is enforced by the k8s schema.
        Version::from_str(&self.current_version).unwrap()
    }

    pub fn target_version(&self) -> Version {
        // TODO This could panic if a custom `brs` is created with improperly specified versions; however, we are removing this
        // attribute in an impending iteration, so we won't fix it.
        Version::from_str(&self.target_version).unwrap()
    }
}
