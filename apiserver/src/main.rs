use apiserver::api::{self, APIServerSettings};
use apiserver::error::{self, Result};
use apiserver::telemetry::init_telemetry;
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

    match run_server().await {
        Err(error) => {
            event!(Level::ERROR, %error, "brupop apiserver failed.");
            fs::write(&termination_log, format!("{}", error))
                .expect("Could not write k8s termination log.");
        }
        Ok(()) => {}
    }

    opentelemetry::global::shutdown_tracer_provider();
}

async fn run_server() -> Result<()> {
    init_telemetry()?;
    let prometheus_exporter = opentelemetry_prometheus::exporter().init();

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreateSnafu)?;

    let settings = APIServerSettings {
        node_client: K8SBottlerocketShadowClient::new(k8s_client.clone()),
        server_port: APISERVER_INTERNAL_PORT as u16,
    };

    api::run_server(settings, k8s_client, Some(prometheus_exporter)).await
}
