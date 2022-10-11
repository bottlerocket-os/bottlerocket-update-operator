use super::{BottlerocketShadowSelector, BottlerocketShadowState};

use snafu::Snafu;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
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
        "Unable to exclude node from load balancer ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    ExcludeNodeFromLB {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketShadowSelector,
    },

    #[snafu(display(
        "IO error occurred while attempting to use APIServerClient: '{}'",
        source
    ))]
    IOError { source: Box<dyn std::error::Error> },

    #[snafu(display(
        "Unable to remove node exclusion from load balancer ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    RemoveNodeExclusionFromLB {
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
        name
    ))]
    NodeWithoutStatus { name: String },

    #[snafu(display(
        "BottlerocketShadow object ('{}') is missing a reference to the owning Node.",
        name
    ))]
    MissingOwnerReference { name: String },

    #[snafu(display(
        "BottlerocketShadow object must have valid rfc3339 timestamp: '{}'",
        source
    ))]
    TimestampFormat { source: chrono::ParseError },
}
