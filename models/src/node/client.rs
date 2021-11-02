use super::error::{self, Result};
use super::{
    BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeSpec, BottlerocketNodeStatus,
    K8S_NODE_KIND,
};
use crate::constants;

use async_trait::async_trait;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::sync::Arc;
use tracing::instrument;

#[cfg(feature = "mockall")]
use mockall::{mock, predicate::*};

#[async_trait]
/// A trait providing an interface to interact with BottlerocketNode objects. This is provided as a trait
/// in order to allow mocks to be used for testing purposes.
pub trait BottlerocketNodeClient: Clone + Sized + Send + Sync {
    /// Create a BottlerocketNode object for the specified node.
    async fn create_node(&self, selector: &BottlerocketNodeSelector) -> Result<BottlerocketNode>;
    /// Update the `.status` of a BottlerocketNode object. Because the single daemon running on each node
    /// uniquely owns its brn object, we allow wholesale overwrites rather than patching.
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<()>;
    /// Update the `.spec` of a BottlerocketNode object.
    // TODO: Does this need to provide helpers for Patching semantics?
    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<()>;
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
        ) -> Result<BottlerocketNode>;
        async fn update_node_status(
            &self,
            selector: &BottlerocketNodeSelector,
            status: &BottlerocketNodeStatus,
        ) -> Result<()>;
        async fn update_node_spec(
            &self,
            selector: &BottlerocketNodeSelector,
            spec: &BottlerocketNodeSpec,
        ) -> Result<()>;
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
    async fn create_node(&self, selector: &BottlerocketNodeSelector) -> Result<BottlerocketNode> {
        (**self).create_node(selector).await
    }
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<()> {
        (**self).update_node_status(selector, status).await
    }

    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<()> {
        (**self).update_node_spec(selector, spec).await
    }
}

#[derive(Clone)]
/// Concrete implementation of the `BottlerocketNodeClient` trait. This implementation will almost
/// certainly be used in ggany case that isn't a unit test.
pub struct K8SBottlerocketNodeClient {
    k8s_client: kube::client::Client,
}

impl K8SBottlerocketNodeClient {
    pub fn new(k8s_client: kube::client::Client) -> Self {
        K8SBottlerocketNodeClient { k8s_client }
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the status of a BottlerocketNode.
struct BottlerocketNodeStatusPatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    status: BottlerocketNodeStatus,
}

impl Default for BottlerocketNodeStatusPatch {
    fn default() -> Self {
        BottlerocketNodeStatusPatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            status: BottlerocketNodeStatus::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the spec of a BottlerocketNode.
struct BottlerocketNodeSpecPatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    spec: BottlerocketNodeSpec,
}

impl Default for BottlerocketNodeSpecPatch {
    fn default() -> Self {
        BottlerocketNodeSpecPatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            spec: BottlerocketNodeSpec::default(),
        }
    }
}

#[async_trait]
impl BottlerocketNodeClient for K8SBottlerocketNodeClient {
    #[instrument(skip(self), err)]
    async fn create_node(&self, selector: &BottlerocketNodeSelector) -> Result<BottlerocketNode> {
        let br_node = BottlerocketNode {
            metadata: ObjectMeta {
                name: Some(selector.brn_resource_name()),
                owner_references: Some(vec![OwnerReference {
                    api_version: "v1".to_string(),
                    kind: "Node".to_string(),
                    name: selector.node_name.clone(),
                    uid: selector.node_uid.clone(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            spec: BottlerocketNodeSpec::default(),
            ..Default::default()
        };

        Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE)
            .create(&PostParams::default(), &br_node)
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::CreateBottlerocketNode {
                selector: selector.clone(),
            })?;

        Ok(br_node)
    }

    #[instrument(skip(self), err)]
    async fn update_node_status(
        &self,
        selector: &BottlerocketNodeSelector,
        status: &BottlerocketNodeStatus,
    ) -> Result<()> {
        let br_node_status_patch = BottlerocketNodeStatusPatch {
            status: status.clone(),
            ..Default::default()
        };

        let br_node_status_patch =
            serde_json::to_value(br_node_status_patch).context(error::CreateK8SPatch)?;

        let api: Api<BottlerocketNode> =
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

        api.patch_status(
            &selector.brn_resource_name(),
            &PatchParams::default(),
            &Patch::Merge(&br_node_status_patch),
        )
        .await
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
        .context(error::UpdateBottlerocketNodeStatus {
            selector: selector.clone(),
        })?;

        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn update_node_spec(
        &self,
        selector: &BottlerocketNodeSelector,
        spec: &BottlerocketNodeSpec,
    ) -> Result<()> {
        let br_node_spec_patch = BottlerocketNodeSpecPatch {
            spec: spec.clone(),
            ..Default::default()
        };
        let br_node_spec_patch =
            serde_json::to_value(br_node_spec_patch).context(error::CreateK8SPatch)?;

        let api: Api<BottlerocketNode> =
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

        api.patch(
            &selector.brn_resource_name(),
            &PatchParams::default(),
            &Patch::Merge(&br_node_spec_patch),
        )
        .await
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
        .context(error::UpdateBottlerocketNodeSpec {
            selector: selector.clone(),
        })?;
        Ok(())
    }
}
