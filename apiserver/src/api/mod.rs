//! This module contains the brupop API server. Endpoints are stored in submodules, separated
//! by the resource on which they act.
mod drain;
mod node;
mod ping;

use crate::{
    auth::{K8STokenAuthorizor, K8STokenReviewer, TokenAuthMiddleware},
    constants::{
        CRD_CONVERT_ENDPOINT, EXCLUDE_NODE_FROM_LB_ENDPOINT, HEADER_BRUPOP_K8S_AUTH_TOKEN,
        HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID, NODE_CORDON_AND_DRAIN_ENDPOINT,
        NODE_RESOURCE_ENDPOINT, NODE_UNCORDON_ENDPOINT, REMOVE_NODE_EXCLUSION_TO_LB_ENDPOINT,
    },
    error::{self, Result},
    telemetry,
};
use models::constants::{
    AGENT, APISERVER_HEALTH_CHECK_ROUTE, APISERVER_SERVICE_NAME, CA_NAME, LABEL_COMPONENT,
    NAMESPACE, PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, TLS_KEY_MOUNT_PATH,
};
use models::node::{BottlerocketShadowClient, BottlerocketShadowSelector};

use actix_web::{
    dev::ServiceRequest,
    http::{self, HeaderMap},
    web::{self, Data},
    App, HttpServer,
};
use actix_web_opentelemetry::RequestMetrics;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, ListParams},
    runtime::{reflector, utils::try_flatten_touched, watcher::watcher},
    ResourceExt,
};
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};
use opentelemetry::global::meter;
use snafu::{OptionExt, ResultExt};
use std::env;
use tracing::{event, Level};
use tracing_actix_web::TracingLogger;

use std::convert::TryFrom;

// The set of API endpoints for which `tracing::Span`s will not be recorded.
pub const NO_TELEMETRY_ENDPOINTS: &[&str] = &[APISERVER_HEALTH_CHECK_ROUTE];

/// A struct containing information intended to be passed to the apiserver via HTTP headers.
pub(crate) struct ApiserverCommonHeaders {
    pub node_selector: BottlerocketShadowSelector,
    pub k8s_auth_token: String,
}

/// Returns a string value extracted from HTTP headers.
fn extract_header_string(headers: &HeaderMap, key: &'static str) -> Result<String> {
    Ok(headers
        .get(key)
        .context(error::HTTPHeaderParse {
            missing_header: key,
        })?
        .to_str()
        .map_err(|_| error::Error::HTTPHeaderParse {
            missing_header: key,
        })?
        .to_string())
}

impl TryFrom<&HeaderMap> for ApiserverCommonHeaders {
    type Error = error::Error;

    fn try_from(headers: &HeaderMap) -> Result<Self> {
        let node_name = extract_header_string(headers, HEADER_BRUPOP_NODE_NAME)?;
        let node_uid = extract_header_string(headers, HEADER_BRUPOP_NODE_UID)?;
        let k8s_auth_token = extract_header_string(headers, HEADER_BRUPOP_K8S_AUTH_TOKEN)?;

        Ok(ApiserverCommonHeaders {
            node_selector: BottlerocketShadowSelector {
                node_name,
                node_uid,
            },
            k8s_auth_token,
        })
    }
}

#[derive(Clone)]
/// Settings that are applied to the apiserver. These settings are provided to each HTTP route
/// via actix's application data system.
pub struct APIServerSettings<T: BottlerocketShadowClient> {
    pub node_client: T,
    pub server_port: u16,
}

/// Runs the apiserver using the given settings.
pub async fn run_server<T: 'static + BottlerocketShadowClient>(
    settings: APIServerSettings<T>,
    k8s_client: kube::Client,
    prometheus_exporter: Option<opentelemetry_prometheus::PrometheusExporter>,
) -> Result<()> {
    let server_port = settings.server_port;

    // Set up a reflector to watch all kubernetes pods in the namespace.
    // We use this information to authenticate write requests from brupop agents.
    let pods = Api::<Pod>::namespaced(k8s_client.clone(), NAMESPACE);

    let pod_store = reflector::store::Writer::<Pod>::default();
    let pod_reader = pod_store.as_reader();

    let pod_reflector = reflector::reflector(
        pod_store,
        watcher(
            pods,
            ListParams::default().labels(&format!("{}={}", LABEL_COMPONENT, AGENT)),
        ),
    );
    let drainer = try_flatten_touched(pod_reflector)
        .filter_map(|x| async move {
            if let Err(err) = &x {
                event!(Level::ERROR, %err, "Failed to process a Pod event");
            }
            std::result::Result::ok(x)
        })
        .for_each(|pod| {
            event!(Level::TRACE, pod_name = %pod.name(), ?pod.spec, ?pod.status, "Processed event for Pod");
            futures::future::ready(())
        });

    // Set up prometheus metrics
    let request_metrics = RequestMetrics::new(
        meter("apiserver"),
        Some(|req: &ServiceRequest| req.path() == "/metrics" && req.method() == http::Method::GET),
        prometheus_exporter,
    );

    // Set up the actix server.

    // Use IP for KUBERNETES_SERVICE_HOST to decide the IP family for the cluster,
    // Match API server IP family same as cluster
    let k8s_service_addr =
        env::var("KUBERNETES_SERVICE_HOST").context(error::MissingClusterIPFamiliy)?;
    let server_addr = if k8s_service_addr.contains(":") {
        // IPv6 format
        format!("[::]:{}", server_port)
    } else {
        // IPv4 format
        format!("0.0.0.0:{}", server_port)
    };

    event!(Level::DEBUG, ?server_addr, "Server addr localhost.");

    let mut builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls()).context(error::SSLError)?;

    builder
        .set_certificate_chain_file(format!("{}/{}", TLS_KEY_MOUNT_PATH, CA_NAME))
        .context(error::SSLError)?;
    builder
        .set_certificate_file(
            format!("{}/{}", TLS_KEY_MOUNT_PATH, PUBLIC_KEY_NAME),
            SslFiletype::PEM,
        )
        .context(error::SSLError)?;
    builder
        .set_private_key_file(
            format!("{}/{}", TLS_KEY_MOUNT_PATH, PRIVATE_KEY_NAME),
            SslFiletype::PEM,
        )
        .context(error::SSLError)?;

    let server = HttpServer::new(move || {
        App::new()
            .wrap(
                TokenAuthMiddleware::new(K8STokenAuthorizor::new(
                    K8STokenReviewer::new(k8s_client.clone()),
                    NAMESPACE.to_string(),
                    pod_reader.clone(),
                    Some(vec![APISERVER_SERVICE_NAME.to_string()]),
                ))
                .exclude(APISERVER_HEALTH_CHECK_ROUTE)
                .exclude(CRD_CONVERT_ENDPOINT),
            )
            .wrap(request_metrics.clone())
            .wrap(TracingLogger::<telemetry::BrupopApiserverRootSpanBuilder>::new())
            .app_data(Data::new(settings.clone()))
            .service(
                web::resource(NODE_RESOURCE_ENDPOINT)
                    .route(web::post().to(node::create_bottlerocket_shadow_resource::<T>))
                    .route(web::put().to(node::update_bottlerocket_shadow_resource::<T>)),
            )
            .service(
                web::resource(NODE_CORDON_AND_DRAIN_ENDPOINT)
                    .route(web::post().to(drain::cordon_and_drain::<T>)),
            )
            .service(
                web::resource(NODE_UNCORDON_ENDPOINT).route(web::post().to(drain::uncordon::<T>)),
            )
            .service(
                web::resource(EXCLUDE_NODE_FROM_LB_ENDPOINT)
                    .route(web::post().to(drain::exclude::<T>)),
            )
            .service(
                web::resource(REMOVE_NODE_EXCLUSION_TO_LB_ENDPOINT)
                    .route(web::post().to(drain::remove_exclusion::<T>)),
            )
            .service(
                web::resource(CRD_CONVERT_ENDPOINT)
                    .route(web::post().to(node::convert_bottlerocket_shadow_resource)),
            )
            .route(
                APISERVER_HEALTH_CHECK_ROUTE,
                web::get().to(ping::health_check),
            )
    })
    .bind_openssl(server_addr, builder)
    .context(error::HttpServerError)?
    .run();

    tokio::select! {
        _ = drainer => {
            event!(Level::ERROR, "reflector drained");
            return Err(error::Error::KubernetesWatcherFailed {});
        },
        res = server => {
            event!(Level::ERROR, "server exited");
            res.context(error::HttpServerError)?;
        },
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::constants::APISERVER_INTERNAL_PORT;
    use models::node::MockBottlerocketShadowClient;

    use std::sync::Arc;

    /// Helper method for tests which can set mock expectations for an API server.
    pub(crate) fn test_settings<F>(
        mock_expectations: F,
    ) -> APIServerSettings<Arc<MockBottlerocketShadowClient>>
    where
        F: FnOnce(&mut MockBottlerocketShadowClient),
    {
        let mut node_client = MockBottlerocketShadowClient::new();
        mock_expectations(&mut node_client);

        // Construct an Arc around node_client so that we can share a reference to the
        // client used in the mock server.
        let node_client = Arc::new(node_client);

        APIServerSettings {
            node_client,
            server_port: APISERVER_INTERNAL_PORT as u16,
        }
    }
}
