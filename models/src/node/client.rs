use super::{
    drain,
    error::{self, Result},
};
use super::{
    BottlerocketShadow, BottlerocketShadowSelector, BottlerocketShadowSpec,
    BottlerocketShadowStatus, K8S_NODE_KIND,
};
use crate::constants;

use async_trait::async_trait;
use k8s_openapi::{api::core::v1::Node, apimachinery::pkg::apis::meta::v1::OwnerReference};
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::sync::Arc;
use tracing::instrument;

#[cfg(feature = "mockall")]
use mockall::{mock, predicate::*};

#[async_trait]
/// A trait providing an interface to interact with BottlerocketShadow objects. This is provided as a trait
/// in order to allow mocks to be used for testing purposes.
pub trait BottlerocketShadowClient: Clone + Sized + Send + Sync {
    /// Create a BottlerocketShadow object for the specified node.
    async fn create_node(
        &self,
        selector: &BottlerocketShadowSelector,
    ) -> Result<BottlerocketShadow>;
    /// Update the `.status` of a BottlerocketShadow object. Because the single daemon running on each node
    /// uniquely owns its brs object, we allow wholesale overwrites rather than patching.
    async fn update_node_status(
        &self,
        selector: &BottlerocketShadowSelector,
        status: &BottlerocketShadowStatus,
    ) -> Result<()>;
    /// Update the `.spec` of a BottlerocketShadow object.
    async fn update_node_spec(
        &self,
        selector: &BottlerocketShadowSelector,
        spec: &BottlerocketShadowSpec,
    ) -> Result<()>;
    // Marks the given node as unschedulable, preventing Pods from being deployed onto it.
    async fn cordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
    // Evicts all pods on the given node.
    async fn drain_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
    // Marks the given node as scheduleable, allowing Pods to be deployed onto it.
    async fn uncordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
}

#[cfg(feature = "mockall")]
mock! {
    /// A Mock BottlerocketShadowClient for use in tests.
    pub BottlerocketShadowClient {}
    #[async_trait]
    impl BottlerocketShadowClient for BottlerocketShadowClient {
        async fn create_node(
            &self,
            selector: &BottlerocketShadowSelector,
        ) -> Result<BottlerocketShadow>;
        async fn update_node_status(
            &self,
            selector: &BottlerocketShadowSelector,
            status: &BottlerocketShadowStatus,
        ) -> Result<()>;
        async fn update_node_spec(
            &self,
            selector: &BottlerocketShadowSelector,
            spec: &BottlerocketShadowSpec,
        ) -> Result<()>;
        async fn cordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
        async fn drain_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
        async fn uncordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()>;
    }

    impl Clone for BottlerocketShadowClient {
        fn clone(&self) -> Self;
    }
}

#[async_trait]
impl<T> BottlerocketShadowClient for Arc<T>
where
    T: BottlerocketShadowClient,
{
    async fn create_node(
        &self,
        selector: &BottlerocketShadowSelector,
    ) -> Result<BottlerocketShadow> {
        (**self).create_node(selector).await
    }
    async fn update_node_status(
        &self,
        selector: &BottlerocketShadowSelector,
        status: &BottlerocketShadowStatus,
    ) -> Result<()> {
        (**self).update_node_status(selector, status).await
    }

    async fn update_node_spec(
        &self,
        selector: &BottlerocketShadowSelector,
        spec: &BottlerocketShadowSpec,
    ) -> Result<()> {
        (**self).update_node_spec(selector, spec).await
    }

    async fn cordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        (**self).cordon_node(selector).await
    }

    async fn drain_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        (**self).drain_node(selector).await
    }

    async fn uncordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        (**self).uncordon_node(selector).await
    }
}

#[derive(Clone)]
/// Concrete implementation of the `BottlerocketShadowClient` trait. This implementation will almost
/// certainly be used in any case that isn't a unit test.
pub struct K8SBottlerocketShadowClient {
    k8s_client: kube::client::Client,
}

impl K8SBottlerocketShadowClient {
    pub fn new(k8s_client: kube::client::Client) -> Self {
        K8SBottlerocketShadowClient { k8s_client }
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the status of a BottlerocketShadow.
struct BottlerocketShadowStatusPatch {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    status: BottlerocketShadowStatus,
}

impl Default for BottlerocketShadowStatusPatch {
    fn default() -> Self {
        BottlerocketShadowStatusPatch {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            status: BottlerocketShadowStatus::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// A helper struct used to serialize and send patches to the k8s API to modify the entire spec of a BottlerocketShadow.
struct BottlerocketShadowSpecOverwrite {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    spec: BottlerocketShadowSpec,
}

impl Default for BottlerocketShadowSpecOverwrite {
    fn default() -> Self {
        BottlerocketShadowSpecOverwrite {
            api_version: constants::API_VERSION.to_string(),
            kind: K8S_NODE_KIND.to_string(),
            spec: BottlerocketShadowSpec::default(),
        }
    }
}

#[async_trait]
impl BottlerocketShadowClient for K8SBottlerocketShadowClient {
    #[instrument(skip(self), err)]
    async fn create_node(
        &self,
        selector: &BottlerocketShadowSelector,
    ) -> Result<BottlerocketShadow> {
        let br_node = BottlerocketShadow {
            metadata: ObjectMeta {
                name: Some(selector.brs_resource_name()),
                owner_references: Some(vec![OwnerReference {
                    api_version: "v1".to_string(),
                    kind: "Node".to_string(),
                    name: selector.node_name.clone(),
                    uid: selector.node_uid.clone(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            spec: BottlerocketShadowSpec::default(),
            ..Default::default()
        };

        Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE)
            .create(&PostParams::default(), &br_node)
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::CreateBottlerocketShadow {
                selector: selector.clone(),
            })?;

        Ok(br_node)
    }

    #[instrument(skip(self), err)]
    async fn update_node_status(
        &self,
        selector: &BottlerocketShadowSelector,
        status: &BottlerocketShadowStatus,
    ) -> Result<()> {
        let br_node_status_patch = BottlerocketShadowStatusPatch {
            status: status.clone(),
            ..Default::default()
        };

        let api: Api<BottlerocketShadow> =
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

        api.patch_status(
            &selector.brs_resource_name(),
            &PatchParams::default(),
            &Patch::Merge(&br_node_status_patch),
        )
        .await
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
        .context(error::UpdateBottlerocketShadowStatus {
            selector: selector.clone(),
        })?;

        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn update_node_spec(
        &self,
        selector: &BottlerocketShadowSelector,
        spec: &BottlerocketShadowSpec,
    ) -> Result<()> {
        let br_node_spec_patch = BottlerocketShadowSpecOverwrite {
            spec: spec.clone(),
            ..Default::default()
        };
        let br_node_spec_patch =
            serde_json::to_value(br_node_spec_patch).context(error::CreateK8SPatch)?;

        let api: Api<BottlerocketShadow> =
            Api::namespaced(self.k8s_client.clone(), constants::NAMESPACE);

        api.patch(
            &selector.brs_resource_name(),
            &PatchParams::default(),
            &Patch::Merge(&br_node_spec_patch),
        )
        .await
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
        .context(error::UpdateBottlerocketShadowSpec {
            selector: selector.clone(),
        })?;
        Ok(())
    }

    /// Marks the given node as unschedulable, preventing Pods from being deployed onto it.
    #[instrument(skip(self), err)]
    async fn cordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        let nodes: Api<Node> = Api::all(self.k8s_client.clone());
        nodes
            .cordon(&selector.node_name)
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::UpdateBottlerocketShadowSpec {
                selector: selector.clone(),
            })?;

        Ok(())
    }

    /// Evicts all pods on the given node.
    #[instrument(skip(self), err)]
    async fn drain_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        drain::drain_node(&self.k8s_client, &selector.node_name)
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::DrainBottlerocketShadow {
                selector: selector.clone(),
            })?;
        Ok(())
    }

    /// Marks the given node as scheduleable, allowing Pods to be deployed onto it.
    #[instrument(skip(self), err)]
    async fn uncordon_node(&self, selector: &BottlerocketShadowSelector) -> Result<()> {
        let nodes: Api<Node> = Api::all(self.k8s_client.clone());
        nodes
            .uncordon(&selector.node_name)
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::UncordonBottlerocketShadow {
                selector: selector.clone(),
            })?;

        Ok(())
    }
}
