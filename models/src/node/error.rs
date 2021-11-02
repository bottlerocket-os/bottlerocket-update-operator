use super::{BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeState};

use kube::ResourceExt;
use snafu::Snafu;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
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

    #[snafu(display(
        "Attempted to perform an operation on a statusless node ({}) which requires a status.",
        brn.metadata.name.as_ref().unwrap_or(&"<no name set>".to_string())
    ))]
    NodeWithoutStatus { brn: BottlerocketNode },

    #[snafu(display("BottlerocketNode object ('{}') is missing a reference to the owning Node.", brn.name()))]
    MissingOwnerReference { brn: BottlerocketNode },
}
