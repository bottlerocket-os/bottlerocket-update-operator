pub mod error;
/// A package aim to enable migrations to new Custom Resource Definitions.
/// Each edition in BottlerocketShadowSpec, BottlerocketShadowState, BottlerocketShadowStatus
/// should result into a new version for BottlerocketShadow.
///
/// Edit the following line with the latest BottlerocketShadow version so that
/// the new version could be exposed to the rest of the program
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// pub use self::v2::{
///    BottlerocketShadow, BottlerocketShadowSpec, BottlerocketShadowState, BottlerocketShadowStatus,
/// };
/// ```
///
/// Push the BottlerocketShadow version to the end of BOTTLEROCKETSHADOW_CRD_METHODS
/// so the build tool could find all BottlerocketShadow versions and generate correct
/// yaml file for CustomResrouceDefinition
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// static ref BOTTLEROCKETSHADOW_CRD_METHODS: Vec<fn() -> CustomResourceDefinition> = {
///    // A list of CRD methods for different BottlerocketShadow version
///    // The latest version should be added at the end of the vector.
///    vec![
///            v1::BottlerocketShadow::crd as fn() -> CustomResourceDefinition,
///            v2::BottlerocketShadow::crd as fn() -> CustomResourceDefinition,
///        ]
/// };
/// ```
///
/// Add the BottlerocketShadow version to the end of BOTTLEROCKETSHADOW_CRD_VERSIONS
/// so the webhook conversion could set up proper conversion_review_versions
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// static ref BOTTLEROCKETSHADOW_CRD_VERSIONS: Vec<String> =
///     vec!["v1".to_string(), "v2".to_string()];
/// ```
///
pub mod v1;
pub mod v2;

/// CRD module wide result type
pub type Result<T> = std::result::Result<T, error::Error>;

use self::error::Error;
pub use self::v2::{
    BottlerocketShadow, BottlerocketShadowSpec, BottlerocketShadowState, BottlerocketShadowStatus,
};
use crate::constants::{
    APISERVER_CRD_CONVERT_ENDPOINT, APISERVER_SERVICE_NAME, APISERVER_SERVICE_PORT,
    CERTIFICATE_NAME, NAMESPACE,
};

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceConversion, CustomResourceDefinition, ServiceReference, WebhookClientConfig,
    WebhookConversion,
};
use kube::CustomResourceExt;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Debug;

lazy_static! {
    static ref BOTTLEROCKETSHADOW_CRD_METHODS: Vec<fn() -> CustomResourceDefinition> = {
        vec![
            v1::BottlerocketShadow::crd as fn() -> CustomResourceDefinition,
            v2::BottlerocketShadow::crd as fn() -> CustomResourceDefinition,
        ]
    };
    static ref BOTTLEROCKETSHADOW_CRD_VERSIONS: Vec<String> =
        vec!["v1".to_string(), "v2".to_string()];
}

pub trait BottlerocketShadowResource: kube::ResourceExt {}

pub trait Selector {
    fn selector(&self) -> Result<BottlerocketShadowSelector>;
}

impl<T: BottlerocketShadowResource> Selector for T {
    fn selector(&self) -> Result<BottlerocketShadowSelector> {
        let node_owner = self
            .meta()
            .owner_references
            .as_ref()
            .ok_or(Error::MissingOwnerReference {
                name: self.name_any(),
            })?
            .first()
            .ok_or(Error::MissingOwnerReference {
                name: self.name_any(),
            })?;

        Ok(BottlerocketShadowSelector {
            node_name: node_owner.name.clone(),
            node_uid: node_owner.uid.clone(),
        })
    }
}

/// Indicates the specific k8s node that BottlerocketShadow object is associated with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottlerocketShadowSelector {
    pub node_name: String,
    pub node_uid: String,
}

impl BottlerocketShadowSelector {
    pub fn brs_resource_name(&self) -> String {
        brs_name_from_node_name(&self.node_name)
    }
}

impl fmt::Display for BottlerocketShadowSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.node_name, self.node_uid)
    }
}

pub fn brs_name_from_node_name(node_name: &str) -> String {
    format!("brs-{}", node_name)
}

/// Combine all different versions of custom resources into one CustomeResourceDefinition yaml
/// kube-rs didn't provide a good way to combine CRDs: https://github.com/kube-rs/kube-rs/issues/569
/// In the combination, this method will keep all settings (metadata, apiVersion, etc.) in lastet_crd,
/// and add the spec.versions part in each old_crd to spec.versions part in latest_crd.
/// When adding those old version, the storage value would be set to false,
/// since only one storage true is allowed among all CRD versions.
fn combine_version_in_crds(
    mut latest_crd: CustomResourceDefinition,
    old_crds: Vec<CustomResourceDefinition>,
) -> CustomResourceDefinition {
    for old_crd in old_crds {
        let mut old_versions = old_crd.spec.versions;

        // Adjust storage value via #derive(CustomResource) is supported yet.
        for old_version in &mut old_versions {
            old_version.storage = false;
        }
        latest_crd.spec.versions.append(&mut old_versions);
    }
    latest_crd
}

/// Generate webhook conversion from scratch since k8s_api didn't provide
/// a decent way to set up CustomResourceConversion
///
/// Sample generated config:
/// conversion:
///   strategy: Webhook
///   webhook:
///     clientConfig:
///       service:
///         name: brupop-apiserver
///         namespace: brupop-bottlerocket-aws
///         path: /crdconvert
///         port: 443
///     conversionReviewVersions:
///       - v1
///       - v2
fn generate_webhook_conversion() -> CustomResourceConversion {
    CustomResourceConversion {
        strategy: "Webhook".to_string(),
        webhook: Some(WebhookConversion {
            client_config: Some(WebhookClientConfig {
                service: Some(ServiceReference {
                    name: APISERVER_SERVICE_NAME.to_string(),
                    namespace: NAMESPACE.to_string(),
                    path: Some(APISERVER_CRD_CONVERT_ENDPOINT.to_string()),
                    port: Some(APISERVER_SERVICE_PORT),
                }),
                ..Default::default()
            }),
            conversion_review_versions: BOTTLEROCKETSHADOW_CRD_VERSIONS.to_vec(),
        }),
    }
}

/// Generate cert-manager annotations to help inject caBundle for webhook
/// https://cert-manager.io/docs/concepts/ca-injector/#injecting-ca-data-from-a-certificate-resource
fn generate_ca_annotations() -> BTreeMap<String, String> {
    let mut cert_manager_annotations = BTreeMap::new();
    cert_manager_annotations.insert(
        "cert-manager.io/inject-ca-from".to_string(),
        format!(
            "{namespace}/{object}",
            namespace = NAMESPACE,
            object = CERTIFICATE_NAME
        ),
    );
    cert_manager_annotations
}

/// Setup webhook conversion and add caBundle
fn add_webhook_setting(
    mut combined_version_crds: CustomResourceDefinition,
) -> CustomResourceDefinition {
    combined_version_crds.spec.conversion = Some(generate_webhook_conversion());
    combined_version_crds.metadata.annotations = Some(generate_ca_annotations());
    combined_version_crds
}

/// `#[derive(CustomResource)]` set default categories to empty list
/// causes mismatch in Kubernetes's object and YAML manifest file,
/// futher causes ArgoCD/FluxCD constantly reapply defined manifest.
fn remove_empty_categories(mut crds: CustomResourceDefinition) -> CustomResourceDefinition {
    crds.spec.names.categories = None;
    crds
}

pub fn combined_crds() -> CustomResourceDefinition {
    let mut crds: Vec<CustomResourceDefinition> = BOTTLEROCKETSHADOW_CRD_METHODS
        .iter()
        .map(|crd_method| crd_method())
        .collect();
    let latest_crd = crds.pop().unwrap();
    let combined_version_crds = combine_version_in_crds(latest_crd, crds);
    let crds_with_webhook = add_webhook_setting(combined_version_crds);
    remove_empty_categories(crds_with_webhook)
}
