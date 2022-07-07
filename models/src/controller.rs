use crate::brupop_labels;
use crate::constants::{
    APP_COMPONENT, APP_MANAGED_BY, APP_PART_OF, BRUPOP, BRUPOP_DOMAIN_LIKE_NAME, CONTROLLER,
    CONTROLLER_DEPLOYMENT_NAME, CONTROLLER_INTERNAL_PORT, CONTROLLER_SERVICE_NAME,
    CONTROLLER_SERVICE_PORT, LABEL_COMPONENT, NAMESPACE,
};
use crate::node::{K8S_NODE_PLURAL, K8S_NODE_STATUS};
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy};
use k8s_openapi::api::core::v1::{
    Affinity, Container, LocalObjectReference, NodeAffinity, NodeSelector, NodeSelectorRequirement,
    NodeSelectorTerm, PodSpec, PodTemplateSpec, Service, ServiceAccount, ServicePort, ServiceSpec,
};
use k8s_openapi::api::rbac::v1::{ClusterRole, ClusterRoleBinding, PolicyRule, RoleRef, Subject};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::ObjectMeta;
use maplit::btreemap;

const BRUPOP_CONTROLLER_SERVICE_ACCOUNT: &str = "brupop-controller-service-account";
const BRUPOP_CONTROLLER_CLUSTER_ROLE: &str = "brupop-controller-role";

/// Defines the brupop-controller service account
pub fn controller_service_account() -> ServiceAccount {
    ServiceAccount {
        metadata: ObjectMeta {
            labels: Some(brupop_labels!(CONTROLLER)),
            name: Some(BRUPOP_CONTROLLER_SERVICE_ACCOUNT.to_string()),
            namespace: Some(NAMESPACE.to_string()),
            annotations: Some(btreemap! {
                "kubernetes.io/service-account.name".to_string() => BRUPOP_CONTROLLER_SERVICE_ACCOUNT.to_string()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Defines the brupop-controller cluster role
pub fn controller_cluster_role() -> ClusterRole {
    ClusterRole {
        metadata: ObjectMeta {
            labels: Some(brupop_labels!(CONTROLLER)),
            name: Some(BRUPOP_CONTROLLER_CLUSTER_ROLE.to_string()),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        rules: Some(vec![
            PolicyRule {
                api_groups: Some(vec![BRUPOP_DOMAIN_LIKE_NAME.to_string()]),
                resources: Some(vec![
                    K8S_NODE_PLURAL.to_string(),
                    K8S_NODE_STATUS.to_string(),
                ]),
                verbs: vec!["get", "list", "watch"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
            PolicyRule {
                api_groups: Some(vec![BRUPOP_DOMAIN_LIKE_NAME.to_string()]),
                resources: Some(vec![K8S_NODE_PLURAL.to_string()]),
                verbs: vec!["create", "patch", "update"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
            PolicyRule {
                api_groups: Some(vec!["apps".to_string()]),
                resources: Some(vec!["deployments".to_string()]),
                verbs: vec![
                    "create",
                    "delete",
                    "deletecollection",
                    "get",
                    "list",
                    "patch",
                    "update",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    }
}

/// Defines the brupop-controller cluster role binding
pub fn controller_cluster_role_binding() -> ClusterRoleBinding {
    ClusterRoleBinding {
        metadata: ObjectMeta {
            labels: Some(brupop_labels!(CONTROLLER)),
            name: Some("brupop-controller-role-binding".to_string()),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        role_ref: RoleRef {
            api_group: "rbac.authorization.k8s.io".to_string(),
            kind: "ClusterRole".to_string(),
            name: BRUPOP_CONTROLLER_CLUSTER_ROLE.to_string(),
        },
        subjects: Some(vec![Subject {
            kind: "ServiceAccount".to_string(),
            name: BRUPOP_CONTROLLER_SERVICE_ACCOUNT.to_string(),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        }]),
    }
}

/// Defines the brupop-controller deployment
pub fn controller_deployment(
    brupop_image: String,
    image_pull_secret: Option<String>,
) -> Deployment {
    let image_pull_secrets =
        image_pull_secret.map(|secret| vec![LocalObjectReference { name: Some(secret) }]);

    Deployment {
        metadata: ObjectMeta {
            labels: Some(brupop_labels!(CONTROLLER)),
            name: Some(CONTROLLER_DEPLOYMENT_NAME.to_string()),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(
                    btreemap! { LABEL_COMPONENT.to_string() => CONTROLLER.to_string()},
                ),
                ..Default::default()
            },
            strategy: Some(DeploymentStrategy {
                type_: Some("Recreate".to_string()),
                ..Default::default()
            }),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(btreemap! {
                        LABEL_COMPONENT.to_string() => CONTROLLER.to_string(),
                    }),
                    namespace: Some(NAMESPACE.to_string()),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    affinity: Some(Affinity {
                        node_affinity: Some(NodeAffinity {
                            required_during_scheduling_ignored_during_execution: Some(
                                NodeSelector {
                                    node_selector_terms: vec![NodeSelectorTerm {
                                        match_expressions: Some(vec![
                                            NodeSelectorRequirement {
                                                key: "kubernetes.io/os".to_string(),
                                                operator: "In".to_string(),
                                                values: Some(vec!["linux".to_string()]),
                                            },
                                            NodeSelectorRequirement {
                                                key: "kubernetes.io/arch".to_string(),
                                                operator: "In".to_string(),
                                                // TODO make sure the pod works on arm64 before adding arm64 here.
                                                // https://github.com/bottlerocket-os/bottlerocket-test-system/issues/90
                                                values: Some(vec![
                                                    "amd64".to_string(),
                                                    "arm64".to_string(),
                                                ]),
                                            },
                                        ]),
                                        ..Default::default()
                                    }],
                                },
                            ),
                            ..Default::default()
                        }),
                        pod_anti_affinity: None,
                        ..Default::default()
                    }),
                    containers: vec![Container {
                        image: Some(brupop_image),
                        image_pull_policy: None,
                        name: BRUPOP.to_string(),
                        command: Some(vec!["./controller".to_string()]),
                        ..Default::default()
                    }],
                    image_pull_secrets,
                    service_account_name: Some(BRUPOP_CONTROLLER_SERVICE_ACCOUNT.to_string()),
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn controller_service() -> Service {
    Service {
        metadata: ObjectMeta {
            labels: Some(brupop_labels!(CONTROLLER)),
            name: Some(CONTROLLER_SERVICE_NAME.to_string()),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },

        spec: Some(ServiceSpec {
            selector: Some(btreemap! { LABEL_COMPONENT.to_string() => CONTROLLER.to_string()}),
            ports: Some(vec![ServicePort {
                port: CONTROLLER_SERVICE_PORT,
                target_port: Some(IntOrString::Int(CONTROLLER_INTERNAL_PORT)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}
