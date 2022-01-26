#[cfg(feature = "server")]
pub mod api;
#[cfg(feature = "server")]
mod auth;
#[cfg(feature = "server")]
pub mod error;
#[cfg(feature = "server")]
pub mod telemetry;

#[cfg(feature = "client")]
pub mod client;

pub(crate) mod constants;

use models::node::{BottlerocketShadowSelector, BottlerocketShadowStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node for which a BottlerocketShadow custom resource should be constructed.
pub struct CreateBottlerocketShadowRequest {
    pub node_selector: BottlerocketShadowSelector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes updates to a BottlerocketShadow object's `status`.
pub struct UpdateBottlerocketShadowRequest {
    pub node_selector: BottlerocketShadowSelector,
    pub node_status: BottlerocketShadowStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node which should have its k8s pods drained, and be cordoned to avoid more pods being scheduled..
pub struct CordonAndDrainBottlerocketShadowRequest {
    pub node_selector: BottlerocketShadowSelector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node which should be uncordoned, allowing k8s Pods to be scheduled to it.
pub struct UncordonBottlerocketShadowRequest {
    pub node_selector: BottlerocketShadowSelector,
}
