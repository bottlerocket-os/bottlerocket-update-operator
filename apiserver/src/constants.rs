pub const NODE_RESOURCE_ENDPOINT: &str = "/bottlerocket-node-resource";
pub const NODE_CORDON_AND_DRAIN_ENDPOINT: &str = "/bottlerocket-node-resource/cordon-and-drain";
pub const NODE_UNCORDON_ENDPOINT: &str = "/bottlerocket-node-resource/uncordon";

// Key names for HTTP headers for apiserver.
pub(crate) const HEADER_BRUPOP_NODE_NAME: &str = "BrupopNodeName";
pub(crate) const HEADER_BRUPOP_NODE_UID: &str = "BrupopNodeUid";
pub(crate) const HEADER_BRUPOP_K8S_AUTH_TOKEN: &str = "BrupopK8sAuthToken";
