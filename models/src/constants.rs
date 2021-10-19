/// Helper macro to avoid retyping the base domain-like name of our system when creating further
/// string constants from it. When given no parameters, this returns the base domain-like name of
/// the system. When given a string literal parameter it adds `/parameter` to the end.
#[macro_export]
macro_rules! brupop_domain {
    () => {
        "brupop.bottlerocket.aws"
    };
    ($s:literal) => {
        concat!(brupop_domain!(), "/", $s)
    };
}

pub const API_VERSION: &str = brupop_domain!("v1");
pub const NAMESPACE: &str = "brupop-bottlerocket-aws";
pub const BRUPOP: &str = "brupop";
pub const BRUPOP_DOMAIN_LIKE_NAME: &str = brupop_domain!();

// Label keys
pub const LABEL_COMPONENT: &str = brupop_domain!("component");

// Standard tags https://kubernetes.io/docs/concepts/overview/working-with-objects/common-labels/
pub const APP_NAME: &str = "app.kubernetes.io/name";
pub const APP_INSTANCE: &str = "app.kubernetes.io/instance";
pub const APP_COMPONENT: &str = "app.kubernetes.io/component";
pub const APP_PART_OF: &str = "app.kubernetes.io/part-of";
pub const APP_MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const APP_CREATED_BY: &str = "app.kubernetes.io/created-by";

// apiserver constants
pub const APISERVER: &str = "apiserver";
pub const APISERVER_INTERNAL_PORT: i32 = 8080; // The internal port on which the the apiservice is hosted.
pub const APISERVER_SERVICE_PORT: i32 = 80; // The k8s service port hosting the apiserver.
pub const APISERVER_MAX_UNAVAILABLE: &str = "33%"; // The maximum number of unavailable nodes for the apiserver deployment.
pub const APISERVER_HEALTH_CHECK_ROUTE: &str = "/ping"; // Route used for apiserver k8s liveness and readiness checks.
pub const APISERVER_SERVICE_NAME: &str = "brupop-apiserver"; // The name for the `svc` fronting the apiserver.
