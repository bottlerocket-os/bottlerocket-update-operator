#[cfg(feature = "server")]
pub mod api;
#[cfg(feature = "server")]
pub mod error;
#[cfg(feature = "server")]
pub mod telemetry;

#[cfg(feature = "client")]
pub mod client;

pub(crate) mod constants;

use models::node::{BottlerocketNodeSelector, BottlerocketNodeStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node for which a BottlerocketNode custom resource should be constructed.
pub struct CreateBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes updates to a BottlerocketNode object's `status`.
pub struct UpdateBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
    pub node_status: BottlerocketNodeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node which should have its k8s pods drained, and be cordoned to avoid more pods being scheduled..
pub struct DrainAndCordonBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node which should be uncordoned, allowing k8s Pods to be scheduled to it.
pub struct UncordonBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
}
