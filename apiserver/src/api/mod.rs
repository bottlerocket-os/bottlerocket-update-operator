//! This module contains the brupop API server. Endpoints are stored in submodules, separated
//! by the resource on which they act.
mod drain;
pub mod error;
mod node;
mod ping;

use crate::{
    auth::{K8STokenAuthorizor, K8STokenReviewer, TokenAuthMiddleware},
    constants::{
        CRD_CONVERT_ENDPOINT, EXCLUDE_NODE_FROM_LB_ENDPOINT, HEADER_BRUPOP_K8S_AUTH_TOKEN,
        HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID, NODE_CORDON_AND_DRAIN_ENDPOINT,
        NODE_RESOURCE_ENDPOINT, NODE_UNCORDON_ENDPOINT, REMOVE_NODE_EXCLUSION_TO_LB_ENDPOINT,
    },
    telemetry,
};
use models::constants::{
    AGENT, APISERVER_HEALTH_CHECK_ROUTE, APISERVER_SERVICE_NAME, CA_NAME, LABEL_COMPONENT,
    PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, TLS_KEY_MOUNT_PATH,
};
use models::node::{read_certificate, BottlerocketShadowClient, BottlerocketShadowSelector};

use actix_web::{
    dev::ServerHandle,
    http::header::HeaderMap,
    web::{self, Data},
    App, HttpServer,
};
use actix_web_opentelemetry::{PrometheusMetricsHandler, RequestMetricsBuilder, RequestTracing};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::Api,
    runtime::{
        reflector,
        watcher::{watcher, Config},
        WatchStreamExt,
    },
    ResourceExt,
};
use opentelemetry::global::meter;
use rustls::{
    server::AllowAnyAnonymousOrAuthenticatedClient, Certificate, PrivateKey, RootCertStore,
    ServerConfig,
};
use rustls_pemfile::{certs, pkcs8_private_keys};
use snafu::{OptionExt, ResultExt};
use std::{env, fs::File, io::BufReader};
use tokio::time::{sleep, Duration};
use tracing::{event, Level};
use tracing_actix_web::TracingLogger;

use std::convert::TryFrom;

// The set of API endpoints for which `tracing::Span`s will not be recorded.
pub const NO_TELEMETRY_ENDPOINTS: &[&str] = &[APISERVER_HEALTH_CHECK_ROUTE];

const CERTIFICATE_DETECTOR_SLEEP_DURATION: Duration = Duration::from_secs(60);

/// The API module-wide result type.
type Result<T> = std::result::Result<T, error::Error>;

/// A struct containing information intended to be passed to the apiserver via HTTP headers.
pub(crate) struct ApiserverCommonHeaders {
    pub node_selector: BottlerocketShadowSelector,
    pub k8s_auth_token: String,
}

/// Returns a string value extracted from HTTP headers.
fn extract_header_string(headers: &HeaderMap, key: &'static str) -> Result<String> {
    Ok(headers
        .get(key)
        .context(error::HTTPHeaderParseSnafu {
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
    pub namespace: String,
}

/// Runs the apiserver using the given settings.
pub async fn run_server<T: 'static + BottlerocketShadowClient>(
    settings: APIServerSettings<T>,
    k8s_client: kube::Client,
    prometheus_exporter: opentelemetry_prometheus::PrometheusExporter,
) -> Result<()> {
    let public_key_path = format!("{}/{}", TLS_KEY_MOUNT_PATH, PUBLIC_KEY_NAME);
    let certificate_cache =
        read_certificate(&public_key_path).context(error::ReadCertificateFailedSnafu)?;

    let server_port = settings.server_port;

    // Set up a reflector to watch all kubernetes pods in the namespace.
    // We use this information to authenticate write requests from brupop agents.
    let pods = Api::<Pod>::namespaced(k8s_client.clone(), &settings.namespace);

    let pod_store = reflector::store::Writer::<Pod>::default();
    let pod_reader = pod_store.as_reader();

    let pod_reflector = reflector::reflector(
        pod_store,
        watcher(
            pods,
            Config::default().labels(&format!("{}={}", LABEL_COMPONENT, AGENT)),
        ),
    );
    let drainer = pod_reflector.touched_objects()
        .filter_map(|x| async move {
            if let Err(err) = &x {
                event!(Level::ERROR, %err, "Failed to process a Pod event");
            }
            std::result::Result::ok(x)
        })
        .for_each(|pod| {
            event!(Level::TRACE, pod_name = %pod.name_any(), ?pod.spec, ?pod.status, "Processed event for Pod");
            futures::future::ready(())
        });

    // Build the metrics meter
    let apiserver_meter = meter("apiserver");

    // Set up metrics request builder
    let request_metrics = RequestMetricsBuilder::new().build(apiserver_meter);

    // Set up the actix server.

    // Use IP for KUBERNETES_SERVICE_HOST to decide the IP family for the cluster,
    // Match API server IP family same as cluster
    let k8s_service_addr =
        env::var("KUBERNETES_SERVICE_HOST").context(error::MissingClusterIPFamilySnafu)?;
    let server_addr = if k8s_service_addr.contains(':') {
        // IPv6 format
        format!("[::]:{}", server_port)
    } else {
        // IPv4 format
        format!("0.0.0.0:{}", server_port)
    };

    event!(Level::DEBUG, ?server_addr, "Server addr localhost.");

    // Server public certificate file
    let cert_file_path = format!("{}/{}", TLS_KEY_MOUNT_PATH, PUBLIC_KEY_NAME);
    let cert_file =
        &mut BufReader::new(File::open(&cert_file_path).context(error::FileOpenSnafu {
            path: cert_file_path.to_string(),
        })?);

    // Private key file
    let key_file_path = format!("{}/{}", TLS_KEY_MOUNT_PATH, PRIVATE_KEY_NAME);
    let key_file =
        &mut BufReader::new(File::open(&key_file_path).context(error::FileOpenSnafu {
            path: key_file_path.to_string(),
        })?);

    // Certificate authority file so a client can authenticate the server
    let ca_file_path = format!("{}/{}", TLS_KEY_MOUNT_PATH, CA_NAME);
    let ca_file = &mut BufReader::new(File::open(&ca_file_path).context(error::FileOpenSnafu {
        path: ca_file_path.to_string(),
    })?);

    // convert files to key/cert objects
    let cert_chain = certs(cert_file)
        .context(error::CertExtractSnafu {
            path: cert_file_path.to_string(),
        })?
        .into_iter()
        .map(Certificate)
        .collect();
    let mut keys: Vec<PrivateKey> = pkcs8_private_keys(key_file)
        .context(error::CertExtractSnafu {
            path: key_file_path.to_string(),
        })?
        .into_iter()
        .map(PrivateKey)
        .collect();
    let cas: Vec<Certificate> = certs(ca_file)
        .context(error::CertExtractSnafu {
            path: ca_file_path.to_string(),
        })?
        .into_iter()
        .map(Certificate)
        .collect();

    let mut cert_store = RootCertStore::empty();
    for ca in cas {
        cert_store.add(&ca).context(error::CertStoreSnafu)?;
    }

    let verifier = AllowAnyAnonymousOrAuthenticatedClient::new(cert_store);

    let tls_config_builder = ServerConfig::builder()
        .with_safe_defaults()
        .with_client_cert_verifier(verifier);

    let tls_config = tls_config_builder
        .with_single_cert(cert_chain, keys.remove(0))
        .context(error::TLSConfigBuildSnafu)
        .unwrap();

    let server = HttpServer::new(move || {
        App::new()
            .wrap(
                TokenAuthMiddleware::new(K8STokenAuthorizor::new(
                    K8STokenReviewer::new(k8s_client.clone()),
                    settings.namespace.to_string(),
                    pod_reader.clone(),
                    Some(vec![APISERVER_SERVICE_NAME.to_string()]),
                ))
                .exclude(APISERVER_HEALTH_CHECK_ROUTE)
                .exclude(CRD_CONVERT_ENDPOINT),
            )
            .wrap(RequestTracing::new())
            .wrap(request_metrics.clone())
            .route(
                "/metrics",
                web::get().to(PrometheusMetricsHandler::new(prometheus_exporter.clone())),
            )
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
    .bind_rustls(server_addr, tls_config)
    .context(error::HttpServerSnafu)?
    .run();

    tokio::select! {
        _ = drainer => {
            event!(Level::ERROR, "reflector drained");
            return Err(error::Error::KubernetesWatcherFailed {});
        },
        _ = reload_certificate(server.handle(), &public_key_path, certificate_cache)=> {
            event!(Level::ERROR, "certificate refreshed");
            return Err(error::Error::ReloadCertificateFailed {});
        },
        res = server => {
            event!(Level::ERROR, "server exited");
            res.context(error::HttpServerSnafu)?;
        },
    };

    Ok(())
}

// The certificate is refreshed periodically (default 60 days). Once the certificate is renewed, the apiserver
// needs to stop in order to reload the new certificate.
// We cache the certificate initially when brupop starts the server, and compare it to the update-to-date certificate periodically.
// If they don't match, we recognize it as a new certificate, so the server needs to be restarted.
async fn reload_certificate(
    server_handler: ServerHandle,
    public_key_path: &str,
    certificate_cache: Vec<u8>,
) -> Result<()> {
    loop {
        let current_certificate =
            read_certificate(public_key_path).context(error::ReadCertificateFailedSnafu)?;
        if current_certificate != certificate_cache {
            event!(
                Level::INFO,
                "Certificate has been renewed, restarting server to reload new certificate"
            );
            server_handler.stop(true).await;
        }
        sleep(CERTIFICATE_DETECTOR_SLEEP_DURATION).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::node::MockBottlerocketShadowClient;

    use std::sync::Arc;

    /// Helper method for tests which can set mock expectations for an API server.
    pub(crate) fn test_settings<F>(
        mock_expectations: F,
    ) -> APIServerSettings<Arc<MockBottlerocketShadowClient>>
    where
        F: FnOnce(&mut MockBottlerocketShadowClient),
    {
        let apiserver_internal_port: i32 = 8443;
        let mut node_client = MockBottlerocketShadowClient::new();
        mock_expectations(&mut node_client);

        // Construct an Arc around node_client so that we can share a reference to the
        // client used in the mock server.
        let node_client = Arc::new(node_client);

        APIServerSettings {
            node_client,
            server_port: apiserver_internal_port as u16,
            namespace: "bottlerocket-update-operator".to_string(),
        }
    }
}
