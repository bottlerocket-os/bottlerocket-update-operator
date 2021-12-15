use controller::{
    error::{self, Result},
    BrupopController,
    telemetry::vending_metrics,
};
use models::{
    constants::{CONTROLLER, NAMESPACE},
    node::{BottlerocketNode, K8SBottlerocketNodeClient},
};

use actix_web::{App, HttpServer, web::Data};

use futures::StreamExt;
use kube::{
    api::{Api, ListParams},
    runtime::{reflector, utils::try_flatten_touched, watcher::watcher},
    ResourceExt,
};
use opentelemetry::sdk::propagation::TraceContextPropagator;
use snafu::ResultExt;
use tracing::{event, Level};
use tracing_bunyan_formatter::{BunyanFormattingLayer, JsonStorageLayer};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

const DEFAULT_TRACE_LEVEL: &str = "info";


#[actix_web::main]
async fn main() -> Result<()> {
    init_telemetry()?;

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    // The `BrupopController` needs a `reflector::Store`, which is updated by a reflector
    // that runs concurrently. We'll create the store and run the reflector here.
    let brns = Api::<BottlerocketNode>::namespaced(k8s_client.clone(), NAMESPACE);
    let brn_store = reflector::store::Writer::<BottlerocketNode>::default();
    let brn_reader = brn_store.as_reader();

    let node_client = K8SBottlerocketNodeClient::new(k8s_client.clone());

    // Setup and run the controller.
    let mut controller = BrupopController::new(node_client, brn_reader);
    let controller_runner = controller.run();

    // Setup and run a reflector, ensuring that `BottlerocketNode` updates are reflected to the controller.
    let brn_reflector = reflector::reflector(brn_store, watcher(brns, ListParams::default()));
    let drainer = try_flatten_touched(brn_reflector)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|brn| {
            event!(
                Level::TRACE,
                brn_name = %brn.name(),
                "Processed a k8s event for a BottlerocketNode object."
            );
            futures::future::ready(())
        });

    // Setup Http server to vend prometheus metrics
    let exporter = opentelemetry_prometheus::exporter().init();
    let prometheus_server = HttpServer::new(move || {
        App::new()
            .app_data(Data::new(exporter.clone()))
            .service(vending_metrics)
    })
    .bind(format!("0.0.0.0:{}", 8080))
    .context(error::PrometheusServerError)?
    .run();

    // TODO if either of these fails, we should write to the k8s termination log and exit.
    tokio::select! {
        _ = drainer => {
            event!(Level::ERROR, "reflector drained");
        },
        _ = controller_runner => {
            event!(Level::ERROR, "controller exited");
        },
        _ = prometheus_server => {
            event!(Level::ERROR, "metric server exited");
        }
    };
    Ok(())
}

fn init_telemetry() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_TRACE_LEVEL));
    let stdio_formatting_layer = BunyanFormattingLayer::new(CONTROLLER.into(), std::io::stdout);
    let subscriber = Registry::default()
        .with(env_filter)
        .with(JsonStorageLayer)
        .with(stdio_formatting_layer);
    tracing::subscriber::set_global_default(subscriber).context(error::TracingConfiguration)?;

    Ok(())
}
