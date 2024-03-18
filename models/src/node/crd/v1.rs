use super::v2::BottlerocketShadow as BottleRocketShadowV2;
use super::v2::BottlerocketShadowSpec as BottlerocketShadowSpecV2;
use super::v2::BottlerocketShadowState as BottlerocketShadowStateV2;
use super::v2::BottlerocketShadowStatus as BottlerocketShadowStatusV2;
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
use std::str::FromStr;
use tokio::time::Duration;
use validator::Validate;

/// BottlerocketShadowState represents a node's state in the update state machine.
#[derive(Copy, Clone, Serialize, Deserialize, Debug, Eq, PartialEq, JsonSchema, Default)]
pub enum BottlerocketShadowState {
    /// Nodes in this state are waiting for new updates to become available. This is both the starting and terminal state
    /// in the update process.
    #[default]
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
    /// the node seems healthy before marking the update as complete.
    MonitoringUpdate,
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

impl From<BottlerocketShadowStateV2> for BottlerocketShadowState {
    fn from(previous_state: BottlerocketShadowStateV2) -> Self {
        // Note: Mapping v2 -> v1 drops fields in the PerformedUpdate field
        match previous_state {
            BottlerocketShadowStateV2::Idle => Self::Idle,
            BottlerocketShadowStateV2::StagedAndPerformedUpdate => Self::StagedUpdate,
            BottlerocketShadowStateV2::RebootedIntoUpdate => Self::RebootedIntoUpdate,
            BottlerocketShadowStateV2::MonitoringUpdate => Self::MonitoringUpdate,
            BottlerocketShadowStateV2::ErrorReset => Self::MonitoringUpdate,
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

impl From<BottlerocketShadowSpecV2> for BottlerocketShadowSpec {
    fn from(previous_spec: BottlerocketShadowSpecV2) -> Self {
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

impl From<BottlerocketShadowStatusV2> for BottlerocketShadowStatus {
    fn from(previous_status: BottlerocketShadowStatusV2) -> Self {
        Self::new(
            // Note: converting from v2 to v1 drops the crash_count and state_transition_failure_timestamp
            previous_status.current_version(),
            previous_status.target_version(),
            BottlerocketShadowState::from(previous_status.current_state),
        )
    }
}

impl From<BottleRocketShadowV2> for BottlerocketShadow {
    fn from(previous_shadow: BottleRocketShadowV2) -> Self {
        let previous_metadata = previous_shadow.metadata;
        let previous_spec = previous_shadow.spec;
        let previous_status = previous_shadow.status;

        let status = previous_status.map(BottlerocketShadowStatus::from);

        let spec = BottlerocketShadowSpec::from(previous_spec);

        BottlerocketShadow {
            metadata: ObjectMeta {
                // The converted object has to maintain the same name, namespace and uid
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
    use super::BottleRocketShadowV2;
    use super::BottlerocketShadow;
    use super::BottlerocketShadowSpec;
    use super::BottlerocketShadowSpecV2;
    use super::BottlerocketShadowState;
    use super::BottlerocketShadowStateV2;
    use super::BottlerocketShadowStatus;
    use super::BottlerocketShadowStatusV2;
    use serde_json::json;

    #[test]
    fn test_state_convert() {
        let original_target_state = vec![
            (json!("Idle"), json!("Idle")),
            (json!("StagedAndPerformedUpdate"), json!("StagedUpdate")),
            (json!("RebootedIntoUpdate"), json!("RebootedIntoUpdate")),
            (json!("MonitoringUpdate"), json!("MonitoringUpdate")),
            (json!("ErrorReset"), json!("MonitoringUpdate")),
        ];

        for (original, target) in original_target_state.into_iter() {
            let old_state: BottlerocketShadowStateV2 = serde_json::from_value(original).unwrap();
            let new_state = BottlerocketShadowState::from(old_state);
            assert_eq!(serde_json::to_value(new_state).unwrap(), target);
        }
    }

    #[test]
    fn test_spec_convert() {
        let original_target_spec = vec![
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
            let old_spec: BottlerocketShadowSpecV2 = serde_json::from_value(original).unwrap();
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
                "target_version": "1.8.0",
                "crash_count":0,
                "state_transition_failure_timestamp": null,
            }),
            json!({
                "current_state": "RebootedIntoUpdate",
                "current_version": "1.6.0",
                "target_version": "1.8.0"
            }),
        )];

        for (original, target) in original_target_status.into_iter() {
            let old_status: BottlerocketShadowStatusV2 = serde_json::from_value(original).unwrap();
            let new_status = BottlerocketShadowStatus::from(old_status);
            assert_eq!(serde_json::to_value(new_status).unwrap(), target);
        }
    }

    #[test]
    fn test_convert_from_old_version() {
        let original_target_version = vec![(
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
                    "state_transition_timestamp": null,
                    "version": null
                },
                "status": {
                    "current_state": "Idle",
                    "target_version": "1.8.0",
                    "current_version": "1.8.0"
                }
            }),
        )];

        for (original, target) in original_target_version.into_iter() {
            let old_brs: BottleRocketShadowV2 = serde_json::from_value(original).unwrap();
            let new_brs = BottlerocketShadow::from(old_brs);
            let new_version = serde_json::to_value(new_brs).unwrap();
            assert_eq!(new_version, target);
        }
    }
}
