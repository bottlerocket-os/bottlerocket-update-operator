use apiserver::api::{self, APIServerSettings};
use apiserver::error::{self, Result};
use apiserver::telemetry::init_telemetry;
use models::node::K8SBottlerocketNodeClient;
use tracing::{event, Level};

use snafu::ResultExt;

use std::env;
use std::fs;

// By default, errors resulting in termination of the apiserver are written to this file,
// which is the location kubernetes uses by default to surface termination-causing errors.
const TERMINATION_LOG: &str = "/dev/termination-log";

#[actix_web::main]
async fn main() {
    let termination_log = env::var("TERMINATION_LOG").unwrap_or(TERMINATION_LOG.to_string());

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

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    let settings = APIServerSettings {
        node_client: K8SBottlerocketNodeClient::new(k8s_client),
        server_port: 8080,
    };

    api::run_server(settings).await
}
