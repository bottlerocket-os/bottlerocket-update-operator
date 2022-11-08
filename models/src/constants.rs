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
/// Helper macro to generate all brupop resources' common k8s labels.
/// When given a string parameter it assign the value to `APP_COMPONENT` and `LABEL_COMPONENT`
#[macro_export]
macro_rules! brupop_labels {
    ($s:expr) => {
        btreemap! {
            APP_COMPONENT => $s,
            APP_MANAGED_BY => BRUPOP,
            APP_PART_OF => BRUPOP,
            LABEL_COMPONENT => $s,
        }
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    };
}

pub const API_VERSION: &str = brupop_domain!("v2");
pub const NAMESPACE: &str = "brupop-bottlerocket-aws";
pub const BRUPOP: &str = "brupop";
pub const BRUPOP_DOMAIN_LIKE_NAME: &str = brupop_domain!();
pub const LABEL_BRUPOP_INTERFACE_NAME: &str = "bottlerocket.aws/updater-interface-version";
pub const BRUPOP_INTERFACE_VERSION: &str = "2.0.0";

// In name space secret name for SSL communication in API server.
pub const CA_NAME: &str = "ca.crt";
pub const PUBLIC_KEY_NAME: &str = "tls.crt";
pub const PRIVATE_KEY_NAME: &str = "tls.key";
pub const TLS_KEY_MOUNT_PATH: &str = "/etc/brupop-tls-keys";
// Certificate object name
pub const ROOT_CERTIFICATE_NAME: &str = "root-certificate";

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
pub const APISERVER_MAX_UNAVAILABLE: &str = "33%"; // The maximum number of unavailable nodes for the apiserver deployment.
pub const APISERVER_HEALTH_CHECK_ROUTE: &str = "/ping"; // Route used for apiserver k8s liveness and readiness checks.
pub const APISERVER_CRD_CONVERT_ENDPOINT: &str = "/crdconvert"; // Custom Resource convert endpoint
pub const APISERVER_SERVICE_NAME: &str = "brupop-apiserver"; // The name for the `svc` fronting the apiserver.

// agent constants
pub const AGENT: &str = "agent";
pub const AGENT_NAME: &str = "brupop-agent";

// controller constants
pub const CONTROLLER: &str = "brupop-controller";
pub const CONTROLLER_DEPLOYMENT_NAME: &str = "brupop-controller-deployment";
pub const CONTROLLER_SERVICE_NAME: &str = "brupop-controller-server"; // The name for the `svc` fronting the controller.
pub const CONTROLLER_INTERNAL_PORT: i32 = 8080; // The internal port on which the the controller service is hosted.
pub const CONTROLLER_SERVICE_PORT: i32 = 80; // The k8s service port hosting the controller service.
pub const BRUPOP_CONTROLLER_PRIORITY_CLASS: &str = "brupop-controller-high-priority";
pub const BRUPOP_CONTROLLER_PREEMPTION_POLICY: &str = "Never";
// We strategically determine the controller priority class value to be one million,
// since one million presents a high priority value which can enable controller to be scheduled preferentially,
// but not a critical value which takes precedence over customers' critical k8s resources.
pub const BRUPOP_CONTROLLER_PRIORITY_VALUE: i32 = 1000000;
