use apiserver::api::{self, APIServerSettings};
use apiserver::telemetry::init_telemetry;
use apiserver_error::{StartServerSnafu, StartTelemetrySnafu};
use models::constants::APISERVER_INTERNAL_PORT;
use models::node::K8SBottlerocketShadowClient;
use tracing::{event, Level};

use snafu::ResultExt;

use std::env;
use std::fs;

// By default, errors resulting in termination of the apiserver are written to this file,
// which is the location kubernetes uses by default to surface termination-causing errors.
const TERMINATION_LOG: &str = "/dev/termination-log";

#[actix_web::main]
async fn main() {
    let termination_log =
        env::var("TERMINATION_LOG").unwrap_or_else(|_| TERMINATION_LOG.to_string());

    if let Err(error) = run_server().await {
        event!(Level::ERROR, %error, "brupop apiserver failed.");
        fs::write(&termination_log, format!("{}", error))
            .expect("Could not write k8s termination log.");
    }

    opentelemetry::global::shutdown_tracer_provider();
}

async fn run_server() -> Result<(), apiserver_error::Error> {
    init_telemetry().context(StartTelemetrySnafu)?;

    let prometheus_exporter = opentelemetry_prometheus::exporter().init();

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(apiserver_error::K8sClientCreateSnafu)?;

    let settings = APIServerSettings {
        node_client: K8SBottlerocketShadowClient::new(k8s_client.clone()),
        server_port: APISERVER_INTERNAL_PORT as u16,
    };

    api::run_server(settings, k8s_client, Some(prometheus_exporter))
        .await
        .context(StartServerSnafu)
}

pub mod apiserver_error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Unable to create client: '{}'", source))]
        K8sClientCreate { source: kube::Error },

        #[snafu(display("Unable to start API server telemetry: '{}'", source))]
        StartTelemetry {
            source: apiserver::telemetry::telemetry_error::Error,
        },

        #[snafu(display("Unable to start API server: '{}'", source))]
        StartServer {
            source: apiserver::api::error::Error,
        },
    }
}
