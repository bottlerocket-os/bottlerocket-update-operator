use std::convert::TryFrom;
use std::env;
use std::fs;
use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;
use snafu::ResultExt;
use tracing::{event, Level};

use apiserver::api::{self, APIServerSettings};
use apiserver_error::{StartServerSnafu, StartTelemetrySnafu};
use models::node::K8SBottlerocketShadowClient;
use models::telemetry;

// By default, errors resulting in termination of the apiserver are written to this file,
// which is the location kubernetes uses by default to surface termination-causing errors.
const TERMINATION_LOG: &str = "/dev/termination-log";
const APISERVER_INTERNAL_PORT_ENV_VAR: &str = "APISERVER_INTERNAL_PORT";

// Store the meter provider so we can shut it down properly
static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();

/// Custom error handler for OpenTelemetry operations
fn handle_opentelemetry_error(operation: &str, error: impl std::fmt::Display) {
    tracing::error!("OpenTelemetry {} error: {}", operation, error);
}

#[actix_web::main]
async fn main() {
    main_inner().await;

    // Properly shutdown the OpenTelemetry meter provider
    if let Some(provider) = METER_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            handle_opentelemetry_error("shutdown", e);
        }
    }
}

async fn main_inner() {
    let termination_log =
        env::var("TERMINATION_LOG").unwrap_or_else(|_| TERMINATION_LOG.to_string());

    if let Err(error) = models::crypto::install_default_crypto_provider() {
        event!(Level::ERROR, %error);
        fs::write(&termination_log, format!("{}", error))
            .expect("Could not write k8s termination log.");
        return;
    }

    if let Err(error) = run_server().await {
        event!(Level::ERROR, %error, "brupop apiserver failed.");
        fs::write(&termination_log, format!("{}", error))
            .expect("Could not write k8s termination log.");
    }
}

async fn run_server() -> Result<(), apiserver_error::Error> {
    telemetry::init_telemetry_from_env().context(StartTelemetrySnafu)?;

    let prometheus_registry = prometheus::Registry::new();

    let prometheus_exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus_registry.clone())
        .build()
        .context(apiserver_error::PrometheusRegsitrySnafu)?;

    let prometheus_provider = SdkMeterProvider::builder()
        .with_reader(prometheus_exporter)
        .with_resource(
            Resource::builder()
                .with_attribute(KeyValue::new("service.name", "apiserver"))
                .build(),
        )
        .build();

    // Store the provider for shutdown
    METER_PROVIDER
        .set(prometheus_provider.clone())
        .expect("Failed to set meter provider");
    global::set_meter_provider(prometheus_provider);

    let incluster_config =
        kube::Config::incluster_dns().context(apiserver_error::K8sClientConfigSnafu)?;

    // Use the existing incluster client config to infer the current namespace
    let namespace = incluster_config.default_namespace.to_string();

    let k8s_client = kube::client::Client::try_from(incluster_config)
        .context(apiserver_error::K8sClientCreateSnafu)?;

    let internal_port: i32 = env::var(APISERVER_INTERNAL_PORT_ENV_VAR)
        .context(apiserver_error::MissingEnvVariableSnafu {
            variable: APISERVER_INTERNAL_PORT_ENV_VAR.to_string(),
        })?
        .parse()
        .context(apiserver_error::ParesePortSnafu)?;
    event!(Level::INFO, %internal_port, "Started API server with port");

    let settings = APIServerSettings {
        node_client: K8SBottlerocketShadowClient::new(k8s_client.clone(), &namespace),
        server_port: internal_port as u16,
        namespace,
    };

    api::run_server(settings, k8s_client, prometheus_registry)
        .await
        .context(StartServerSnafu)
}

pub mod apiserver_error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Failed to configure tls to use aws_lc for crypto."))]
        CryptoConfigure,

        #[snafu(display(
            "Unable to get environment variable '{}' for API server due to : '{}'",
            variable,
            source
        ))]
        MissingEnvVariable {
            source: std::env::VarError,
            variable: String,
        },

        #[snafu(display("Unable to create kubernetes client config: '{}'", source))]
        K8sClientConfig {
            source: kube::config::InClusterError,
        },

        #[snafu(display("Unable to create client: '{}'", source))]
        K8sClientCreate { source: kube::Error },

        #[snafu(display("Unable to parse internal port: '{}'", source))]
        ParesePort { source: std::num::ParseIntError },

        #[snafu(display("Unable to start API server telemetry: '{}'", source))]
        StartTelemetry {
            source: models::telemetry::TelemetryConfigError,
        },

        #[snafu(display("Unable to start API server: '{}'", source))]
        StartServer {
            source: apiserver::api::error::Error,
        },

        #[snafu(display("Error creating prometheus registry: '{}'", source))]
        PrometheusRegsitry {
            source: opentelemetry_sdk::error::OTelSdkError,
        },
    }
}
