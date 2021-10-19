use crate::constants::NAMESPACE;
use k8s_openapi::api::core::v1::Namespace;
use kube::api::ObjectMeta;
use maplit::btreemap;

/// Defines the brupop namespace
pub fn brupop_namespace() -> Namespace {
    Namespace {
        metadata: ObjectMeta {
            labels: Some(btreemap! {
                "name".to_string() => "brupop".to_string()
            }),
            name: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        spec: None,
        status: None,
    }
}
