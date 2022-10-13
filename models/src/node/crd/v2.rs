use super::v1::BottlerocketShadow as BottleRocketShadowV1;
use super::v1::BottlerocketShadowSpec as BottlerocketShadowSpecV1;
use super::v1::BottlerocketShadowState as BottlerocketShadowStateV1;
use super::v1::BottlerocketShadowStatus as BottlerocketShadowStatusV1;
use super::BottlerocketShadowResource;
use super::{error, Result};
use crate::node::SEMVER_RE;

use chrono::{DateTime, Utc};
use kube::api::ObjectMeta;
use kube::CustomResource;
use schemars::JsonSchema;
pub use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::cmp::Ordering;
use std::convert::From;
use std::str::FromStr;
use tokio::time::Duration;
use validator::Validate;
/// BottlerocketShadowState represents a node's state in the update state machine.
#[derive(Copy, Clone, Serialize, Deserialize, Debug, Eq, PartialEq, JsonSchema)]
pub enum BottlerocketShadowState {
    /// Nodes in this state are waiting for new updates to become available. This is both the starting, terminal and recovery state
    /// in the update process.
    Idle,
    /// Nodes in this state have staged a new update image, have installed the new image, and have updated the partition table
    /// to mark it as the new active image.
    StagedAndPerformedUpdate,
    /// Nodes in this state have used the kubernetes cordon and drain APIs to remove
    /// running pods, have un-cordoned the node to allow work to be scheduled, and
    /// have rebooted after performing an update.
    RebootedIntoUpdate,
    /// Nodes in this state are monitoring to ensure that the node seems healthy before
    /// marking the update as complete.
    MonitoringUpdate,
    /// Nodes in this state have crashed due to Bottlerocket Update API call failure.
    ErrorReset,
}

impl Default for BottlerocketShadowState {
    fn default() -> Self {
        BottlerocketShadowState::Idle
    }
}

// These constants define the maximum amount of time to allow a machine to transition *into* this state.
const STAGED_AND_PERFORMED_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(720));
const REBOOTED_INTO_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(600));
const MONITORING_UPDATE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(300));
const IDLE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));
const ERROR_RESET_TIMEOUT: Option<Duration> = Some(Duration::from_secs(u64::MAX));

impl BottlerocketShadowState {
    /// Returns the next state in the state machine if the current state has been reached successfully.
    pub fn on_success(&self) -> Self {
        match self {
            Self::Idle => Self::StagedAndPerformedUpdate,
            Self::StagedAndPerformedUpdate => Self::RebootedIntoUpdate,
            Self::RebootedIntoUpdate => Self::MonitoringUpdate,
            Self::MonitoringUpdate => Self::Idle,
            Self::ErrorReset => Self::Idle,
        }
    }

    /// Returns the total time that a node can spend transitioning *from* the given state to the next state in the process.
    pub fn timeout_time(&self) -> Option<Duration> {
        match self {
            Self::Idle => IDLE_TIMEOUT,
            Self::StagedAndPerformedUpdate => STAGED_AND_PERFORMED_UPDATE_TIMEOUT,
            Self::RebootedIntoUpdate => REBOOTED_INTO_UPDATE_TIMEOUT,
            Self::MonitoringUpdate => MONITORING_UPDATE_TIMEOUT,
            Self::ErrorReset => ERROR_RESET_TIMEOUT,
        }
    }
}

impl From<BottlerocketShadowStateV1> for BottlerocketShadowState {
    fn from(previous_state: BottlerocketShadowStateV1) -> Self {
        // TODO: Remap the state when merge PR with preventing controller from being unscheduled
        match previous_state {
            BottlerocketShadowStateV1::Idle => Self::Idle,
            BottlerocketShadowStateV1::StagedUpdate => Self::StagedAndPerformedUpdate,
            BottlerocketShadowStateV1::PerformedUpdate => Self::StagedAndPerformedUpdate,
            BottlerocketShadowStateV1::RebootedIntoUpdate => Self::RebootedIntoUpdate,
            BottlerocketShadowStateV1::MonitoringUpdate => Self::MonitoringUpdate,
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
    version = "v2",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.current_state"}"#,
    printcolumn = r#"{"name":"Version", "type":"string", "jsonPath":".status.current_version"}"#,
    printcolumn = r#"{"name":"Target State", "type":"string", "jsonPath":".spec.state"}"#,
    printcolumn = r#"{"name":"Target Version", "type":"string", "jsonPath":".spec.version"}"#,
    printcolumn = r#"{"name":"Crash Count", "type":"string", "jsonPath":".status.crash_count"}"#
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

    /// Returns whether or not a node has crashed.
    pub fn has_crashed(&self) -> bool {
        self.status.as_ref().map_or(false, |node_status| {
            node_status.current_state == BottlerocketShadowState::ErrorReset
        })
    }

    /// Order BottleRocketShadow based on crash_count in status
    /// to determine the priority to be handled by the controller.
    /// Uninitialized status should be considered as lowest priority.
    pub fn compare_crash_count(&self, other: &Self) -> Ordering {
        match (self.status.as_ref(), other.status.as_ref()) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(s1), Some(s2)) => s1.crash_count().cmp(&s2.crash_count()),
        }
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
    pub fn state_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
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

impl From<BottlerocketShadowSpecV1> for BottlerocketShadowSpec {
    fn from(previous_spec: BottlerocketShadowSpecV1) -> Self {
        Self::new(
            BottlerocketShadowState::from(previous_spec.state),
            previous_spec.state_timestamp().unwrap(),
            previous_spec.version(),
        )
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
    crash_count: u32,
    state_transition_failure_timestamp: Option<String>,
}

impl BottlerocketShadowStatus {
    pub fn new(
        current_version: Version,
        target_version: Version,
        current_state: BottlerocketShadowState,
        crash_count: u32,
        state_transition_failure_timestamp: Option<DateTime<Utc>>,
    ) -> Self {
        let state_transition_failure_timestamp =
            state_transition_failure_timestamp.map(|ts| ts.to_rfc3339());
        BottlerocketShadowStatus {
            current_version: current_version.to_string(),
            target_version: target_version.to_string(),
            current_state,
            crash_count,
            state_transition_failure_timestamp,
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

    pub fn crash_count(&self) -> u32 {
        self.crash_count
    }

    /// JsonSchema cannot appropriately handle DateTime objects. This accessor returns the failure transition timestamp
    /// as a DateTime.
    pub fn failure_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        self.state_transition_failure_timestamp
            .as_ref()
            .map(|ts_str| {
                DateTime::parse_from_rfc3339(ts_str)
                    // Convert `DateTime<FixedOffset>` into `DateTime<Utc>`
                    .map(|ts| ts.into())
                    .context(error::TimestampFormatSnafu)
            })
            .transpose()
    }
}

impl From<BottlerocketShadowStatusV1> for BottlerocketShadowStatus {
    fn from(previous_status: BottlerocketShadowStatusV1) -> Self {
        Self::new(
            previous_status.current_version(),
            previous_status.target_version(),
            BottlerocketShadowState::from(previous_status.current_state),
            0,
            None,
        )
    }
}

impl From<BottleRocketShadowV1> for BottlerocketShadow {
    fn from(previous_shadow: BottleRocketShadowV1) -> Self {
        let previous_metadata = previous_shadow.metadata;
        let previous_spec = previous_shadow.spec;
        let previous_status = previous_shadow.status;

        let status = previous_status.map(BottlerocketShadowStatus::from);

        let spec = BottlerocketShadowSpec::from(previous_spec);

        BottlerocketShadow {
            metadata: ObjectMeta {
                /// The converted object has to maintain the same name, namespace and uid
                name: previous_metadata.name,
                namespace: previous_metadata.namespace,
                uid: previous_metadata.uid,
                owner_references: previous_metadata.owner_references,
                ..Default::default()
            },
            spec,
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BottleRocketShadowV1;
    use super::BottlerocketShadow;
    use super::BottlerocketShadowSpec;
    use super::BottlerocketShadowSpecV1;
    use super::BottlerocketShadowState;
    use super::BottlerocketShadowStateV1;
    use super::BottlerocketShadowStatus;
    use super::BottlerocketShadowStatusV1;
    use serde_json::json;

    #[test]
    fn test_state_convert() {
        let original_target_state = vec![
            (json!("Idle"), json!("Idle")),
            (json!("StagedUpdate"), json!("StagedAndPerformedUpdate")),
            (json!("PerformedUpdate"), json!("StagedAndPerformedUpdate")),
            (json!("RebootedIntoUpdate"), json!("RebootedIntoUpdate")),
            (json!("MonitoringUpdate"), json!("MonitoringUpdate")),
        ];

        for (original, target) in original_target_state.into_iter() {
            let old_state: BottlerocketShadowStateV1 = serde_json::from_value(original).unwrap();
            let new_state = BottlerocketShadowState::from(old_state);
            assert_eq!(serde_json::to_value(new_state).unwrap(), target);
        }
    }

    #[test]
    fn test_spec_convert() {
        let original_target_spec = vec![
            (
                json!({
                    "state": "Idle",
                    "state_transition_timestamp": null,
                    "version": null
                }),
                json!({
                    "state": "Idle",
                    "state_transition_timestamp": null,
                    "version": null
                }),
            ),
            (
                json!({
                    "state": "RebootedIntoUpdate",
                    "state_transition_timestamp": "2022-07-09T19:32:38.609610964+00:00",
                    "version": "1.8.0"
                }),
                json!({
                    "state": "RebootedIntoUpdate",
                    "state_transition_timestamp": "2022-07-09T19:32:38.609610964+00:00",
                    "version": "1.8.0"
                }),
            ),
        ];

        for (original, target) in original_target_spec.into_iter() {
            let old_spec: BottlerocketShadowSpecV1 = serde_json::from_value(original).unwrap();
            let new_spec = BottlerocketShadowSpec::from(old_spec);
            assert_eq!(serde_json::to_value(new_spec).unwrap(), target);
        }
    }

    #[test]
    fn test_status_convert() {
        let original_target_status = vec![(
            json!({
                "current_state": "RebootedIntoUpdate",
                "current_version": "1.6.0",
                "target_version": "1.8.0"
            }),
            json!({
                "current_state": "RebootedIntoUpdate",
                "current_version": "1.6.0",
                "target_version": "1.8.0",
                "crash_count":0,
                "state_transition_failure_timestamp": null,
            }),
        )];

        for (original, target) in original_target_status.into_iter() {
            let old_status: BottlerocketShadowStatusV1 = serde_json::from_value(original).unwrap();
            let new_status = BottlerocketShadowStatus::from(old_status);
            assert_eq!(serde_json::to_value(new_status).unwrap(), target);
        }
    }

    #[test]
    fn test_convert_from_old_version() {
        let original_target_version = vec![(
            json!({
                "apiVersion": "brupop.bottlerocket.aws/v1",
                "kind": "BottlerocketShadow",
                "metadata": {
                    "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                    "namespace": "brupop-bottlerocket-aws",
                    "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                    "ownerReferences": [
                        {
                            "apiVersion": "v1",
                            "kind": "Node",
                            "name": "ip-192-168-22-145.us-west-2.compute.internal",
                            "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                        }
                    ]
                },
                "spec": {
                    "state": "Idle",
                },
                "status": {
                    "current_state": "Idle",
                    "target_version": "1.8.0",
                    "current_version": "1.8.0"
                }

            }),
            json!({
                "apiVersion": "brupop.bottlerocket.aws/v2",
                "kind": "BottlerocketShadow",
                "metadata": {
                    "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                    "namespace": "brupop-bottlerocket-aws",
                    "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                    "ownerReferences": [
                        {
                            "apiVersion": "v1",
                            "kind": "Node",
                            "name": "ip-192-168-22-145.us-west-2.compute.internal",
                            "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                        }
                    ]
                },
                "spec": {
                    "state": "Idle",
                    "state_transition_timestamp": null,
                    "version": null
                },
                "status": {
                    "current_state": "Idle",
                    "target_version": "1.8.0",
                    "current_version": "1.8.0",
                    "crash_count": 0,
                    "state_transition_failure_timestamp": null,
                }

            }),
        )];

        for (original, target) in original_target_version.into_iter() {
            let old_brs: BottleRocketShadowV1 = serde_json::from_value(original).unwrap();
            let new_brs = BottlerocketShadow::from(old_brs);
            let new_version = serde_json::to_value(new_brs).unwrap();
            assert_eq!(new_version, target);
        }
    }
}
