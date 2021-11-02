use crate::constants;

use async_trait::async_trait;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tracing::{event, span, Instrument, Level};

use std::sync::Arc;

#[cfg(feature = "mockall")]
use mockall::{mock, predicate::*};

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum BottlerocketNodeError {
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

    #[snafu(display("Unable to create patch to send to Kubernetes API: '{}'", source))]
    CreateK8SPatch { source: serde_json::error::Error },
}

/// BottlerocketNodeState represents a node's state in the update state machine.
#[derive(Clone, Serialize, Deserialize, Debug, Eq, PartialEq, JsonSchema)]
pub enum BottlerocketNodeState {
    WaitingForUpdate,
    PreparingToUpdate,
    PerformingUpdate,
    RebootingToUpdate,
    MonitoringUpdate,
}

impl Default for BottlerocketNodeState {
    fn default() -> Self {
        BottlerocketNodeState::WaitingForUpdate
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
    Clone, CustomResource, Serialize, Deserialize, Debug, Default, Eq, PartialEq, JsonSchema,
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
    state: BottlerocketNodeState,
    version: Option<String>,
}

/// `BottlerocketNodeStatus` surfaces the current state of a bottlerocket node. The status is updated by the host agent,
/// while the spec is updated by the brupop controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq, JsonSchema)]
pub struct BottlerocketNodeStatus {
    pub current_version: String,
    pub available_versions: Vec<String>,
    pub current_state: BottlerocketNodeState,
}

/// Indicates the specific k8s node that BottlerocketNode object is associated with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottlerocketNodeSelector {
    pub node_name: String,
    pub node_uid: String,
}

fn node_resource_name(node_selector: &BottlerocketNodeSelector) -> String {
    format!("brn-{}", node_selector.node_name)
}

#[async_trait]
/// A trait providing an interface to interact with BottlerocketNode objects. This is provided as a trait
/// in order to allow mocks to be used for testing purposes.
pub trait BottlerocketNodeClient: Clone + Sized + Send + Sync {
    /// Create a BottlerocketNode object for the specified node.
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError>;
    /// Update the `.status` of a BottlerocketNode object. Because the single daemon running on each node
    /// uniquely owns its brn object, we allow wholesale overwrites rather than patching.
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError>;
    /// Update the `.spec` of a BottlerocketNode object.
    // TODO: Does this need to provide helpers for Patching semantics?
    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError>;
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the status of a BottlerocketNode.
struct BottlerocketNodePatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    status: BottlerocketNodeStatus,
}

impl Default for BottlerocketNodePatch {
    fn default() -> Self {
        BottlerocketNodePatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            status: BottlerocketNodeStatus::default(),
        }
    }
}

#[cfg(feature = "mockall")]
mock! {
    /// A Mock BottlerocketNodeClient for use in tests.
    pub BottlerocketNodeClient {}
    #[async_trait]
    impl BottlerocketNodeClient for BottlerocketNodeClient {
        async fn create_node(
            &self,
            selector: &BottlerocketNodeSelector,
        ) -> Result<BottlerocketNode, BottlerocketNodeError>;
        async fn update_node_status(
            &self,
            selector: &BottlerocketNodeSelector,
            status: &BottlerocketNodeStatus,
        ) -> Result<(), BottlerocketNodeError>;
        async fn update_node_spec(
            &self,
            selector: &BottlerocketNodeSelector,
            spec: &BottlerocketNodeSpec,
        ) -> Result<(), BottlerocketNodeError>;
    }

    impl Clone for BottlerocketNodeClient {
        fn clone(&self) -> Self;
    }
}

#[async_trait]
impl<T> BottlerocketNodeClient for Arc<T>
where
    T: BottlerocketNodeClient,
{
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError> {
        (**self).create_node(selector).await
    }
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError> {
        (**self).update_node_status(selector, status).await
    }

    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError> {
        (**self).update_node_spec(selector, spec).await
    }
}

#[derive(Clone)]
/// Concrete implementation of the `BottlerocketNodeClient` trait. This implementation will almost
/// certainly be used in any case that isn't a unit test.
pub struct K8SBottlerocketNodeClient {
    k8s_client: kube::client::Client,
}

impl K8SBottlerocketNodeClient {
    pub fn new(k8s_client: kube::client::Client) -> Self {
        K8SBottlerocketNodeClient { k8s_client }
    }
}

#[async_trait]
impl BottlerocketNodeClient for K8SBottlerocketNodeClient {
    async fn create_node(
        &self,
        selector: &BottlerocketNodeSelector,
    ) -> Result<BottlerocketNode, BottlerocketNodeError> {
        let create_span = span!(
            Level::INFO,
            "create_node",
            node_name = %selector.node_name,
            node_uid = %selector.node_uid,
        );

        // Use an asynchronous closure to avoid tracing across an await bound.
        async move {
            let br_node = BottlerocketNode {
                metadata: ObjectMeta {
                    name: Some(node_resource_name(&selector)),
                    owner_references: Some(vec![OwnerReference {
                        api_version: "v1".to_string(),
                        kind: "BottlerocketNode".to_string(),
                        name: selector.node_name.clone(),
                        uid: selector.node_uid.clone(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
                spec: BottlerocketNodeSpec::default(),
                ..Default::default()
            };

            event!(
                Level::INFO,
                "Sending BottlerocketNode create request to kubernetes cluster."
            );
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE)
                .create(&PostParams::default(), &br_node)
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(CreateBottlerocketNode {
                    selector: selector.clone(),
                })?;
            event!(
                Level::INFO,
                "BottlerocketNode create request completed successfully."
            );

            Ok(br_node)
        }
        .instrument(create_span)
        .await
    }

    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<(), BottlerocketNodeError> {
        let br_node_patch = BottlerocketNodePatch {
            status: status.clone(),
            ..Default::default()
        };
        let update_span = span!(
            Level::INFO,
            "update_node_status",
            node_name = %selector.node_name,
            node_uid = %selector.node_uid,
        );

        // Use an asynchronous closure to avoid tracing across an await bound.
        async move {
            let br_node_patch = serde_json::to_value(br_node_patch).context(CreateK8SPatch)?;

            let api: Api<BottlerocketNode> =
                Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

            event!(
                Level::INFO,
                "Sending BottlerocketNode patch request to kubernetes cluster.",
            );
            api.patch_status(
                &node_resource_name(&selector),
                &PatchParams::default(),
                &Patch::Merge(&br_node_patch),
            )
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(UpdateBottlerocketNodeStatus {
                selector: selector.clone(),
            })?;
            event!(
                Level::INFO,
                "BottlerocketNode patch request completed successfully."
            );

            Ok(())
        }
        .instrument(update_span)
        .await
    }

    async fn update_node_spec(
        &self,
        _selector: &BottlerocketNodeSelector,
        _spec: &BottlerocketNodeSpec,
    ) -> Result<(), BottlerocketNodeError> {
        unimplemented!()
    }
}
