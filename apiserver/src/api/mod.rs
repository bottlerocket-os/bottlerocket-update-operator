//! This module contains the brupop API server. Endpoints are stored in submodules, separated
//! by the resource on which they act.
mod node;
mod ping;

use crate::{
    auth::{K8STokenAuthorizor, K8STokenReviewer, TokenAuthMiddleware},
    constants::{
        HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID,
        NODE_RESOURCE_ENDPOINT,
    },
    error::{self, Result},
    telemetry,
};
use models::constants::{
    AGENT, APISERVER_HEALTH_CHECK_ROUTE, APISERVER_SERVICE_NAME, LABEL_COMPONENT, NAMESPACE,
};
use models::node::{BottlerocketNodeClient, BottlerocketNodeSelector};

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
use opentelemetry::global::meter;
use snafu::{OptionExt, ResultExt};
use tracing::{event, Level};
use tracing_actix_web::TracingLogger;

use std::convert::TryFrom;

// The set of API endpoints for which `tracing::Span`s will not be recorded.
pub const NO_TELEMETRY_ENDPOINTS: &[&str] = &[APISERVER_HEALTH_CHECK_ROUTE];

/// A struct containing information intended to be passed to the apiserver via HTTP headers.
pub(crate) struct ApiserverCommonHeaders {
    pub node_selector: BottlerocketNodeSelector,
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
            node_selector: BottlerocketNodeSelector {
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
pub struct APIServerSettings<T: BottlerocketNodeClient> {
    pub node_client: T,
    pub server_port: u16,
}

/// Runs the apiserver using the given settings.
pub async fn run_server<T: 'static + BottlerocketNodeClient>(
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
    let server = HttpServer::new(move || {
        App::new()
            .wrap(
                TokenAuthMiddleware::new(K8STokenAuthorizor::new(
                    K8STokenReviewer::new(k8s_client.clone()),
                    NAMESPACE.to_string(),
                    pod_reader.clone(),
                    Some(vec![APISERVER_SERVICE_NAME.to_string()]),
                ))
                .exclude(APISERVER_HEALTH_CHECK_ROUTE),
            )
            .wrap(request_metrics.clone())
            .wrap(TracingLogger::<telemetry::BrupopApiserverRootSpanBuilder>::new())
            .app_data(Data::new(settings.clone()))
            .service(
                web::resource(NODE_RESOURCE_ENDPOINT)
                    .route(web::post().to(node::create_bottlerocket_node_resource::<T>))
                    .route(web::put().to(node::update_bottlerocket_node_resource::<T>)),
            )
            .route(
                APISERVER_HEALTH_CHECK_ROUTE,
                web::get().to(ping::health_check),
            )
    })
    .bind(format!("0.0.0.0:{}", server_port))
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
    use models::node::MockBottlerocketNodeClient;

    use std::sync::Arc;

    /// Helper method for tests which can set mock expectations for an API server.
    pub(crate) fn test_settings<F>(
        mock_expectations: F,
    ) -> APIServerSettings<Arc<MockBottlerocketNodeClient>>
    where
        F: FnOnce(&mut MockBottlerocketNodeClient),
    {
        let mut node_client = MockBottlerocketNodeClient::new();
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