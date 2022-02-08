use super::{BottlerocketShadow, BottlerocketShadowSelector, BottlerocketShadowState};

use kube::ResourceExt;
use snafu::Snafu;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
    #[snafu(display(
        "Unable to create BottlerocketShadow ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    CreateBottlerocketShadow {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "Unable to update BottlerocketShadow status ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UpdateBottlerocketShadowStatus {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "Unable to update BottlerocketShadow spec ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UpdateBottlerocketShadowSpec {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "Unable to cordon BottlerocketShadow ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    CordonBottlerocketShadow {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "Unable to drain BottlerocketShadow ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    DrainBottlerocketShadow {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "Unable to uncordon BottlerocketShadow ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UncordonBottlerocketShadow {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "BottlerocketShadow does not have a k8s spec ({}, {}).'",
        selector.node_name,
        selector.node_uid
    ))]
    NodeWithoutSpec {
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display("Unable to create patch to send to Kubernetes API: '{}'", source))]
    CreateK8SPatch { source: serde_json::error::Error },

    #[snafu(display("Attempted to progress node state machine without achieving current desired state. Current state: '{:?}'. Desired state: '{:?}'", current_state, desired_state))]
    NodeSpecNotAchieved {
        current_state: BottlerocketShadowState,
        desired_state: BottlerocketShadowState,
    },

    #[snafu(display(
        "Attempted to perform an operation on a statusless node ({}) which requires a status.",
        brs.metadata.name.as_ref().unwrap_or(&"<no name set>".to_string())
    ))]
    NodeWithoutStatus { brs: BottlerocketShadow },

    #[snafu(display("BottlerocketShadow object ('{}') is missing a reference to the owning Node.", brs.name()))]
    MissingOwnerReference { brs: BottlerocketShadow },

    #[snafu(display(
        "BottlerocketShadow object must have valid rfc3339 timestamp: '{}'",
        source
    ))]
    TimestampFormat { source: chrono::ParseError },
}
