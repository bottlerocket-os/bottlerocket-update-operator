use agent::agentclient::BrupopAgent;
use agent::error::{self, Result};
use apiserver::client::K8SAPIServerClient;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use kube::api::ListParams;
use kube::runtime::reflector;
use kube::runtime::utils::try_flatten_touched;
use kube::runtime::watcher::watcher;
use kube::Api;
use models::agent::{AGENT_TOKEN_PATH, TOKEN_PROJECTION_MOUNT_PATH};
use models::constants::{AGENT, NAMESPACE};

use models::node::{brs_name_from_node_name, BottlerocketShadow};

use opentelemetry::sdk::propagation::TraceContextPropagator;
use snafu::{OptionExt, ResultExt};
use tracing::{event, Level};
use tracing_bunyan_formatter::{BunyanFormattingLayer, JsonStorageLayer};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

use std::env;
use std::fs;
use std::path::Path;

const TERMINATION_LOG: &str = "/dev/termination-log";
#[tokio::main]
async fn main() {
    let termination_log = env::var("TERMINATION_LOG").unwrap_or(TERMINATION_LOG.to_string());

    match run_agent().await {
        Err(error) => {
            fs::write(&termination_log, format!("{}", error))
                .expect("Could not write k8s termination log.");
        }
        Ok(()) => {}
    }
}

async fn run_agent() -> Result<()> {
    init_telemetry()?;

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    // Configure our brupop apiserver client to use the auth token mounted to our Pod.
    let token_path = Path::new(TOKEN_PROJECTION_MOUNT_PATH).join(AGENT_TOKEN_PATH);
    let token_path = token_path.to_str().context(error::Assertion {
        message: "Token path (defined in models/agent.rs) is not valid unicode.",
    })?;
    let apiserver_client = K8SAPIServerClient::new(token_path.to_string());

    // Get node and BottlerocketShadow names
    let associated_node_name = env::var("MY_NODE_NAME").context(error::GetNodeName)?;
    let associated_bottlerocketshadow_name = brs_name_from_node_name(&associated_node_name);

    // Generate reflector to watch and cache BottlerocketShadow
    let brss = Api::<BottlerocketShadow>::namespaced(k8s_client.clone(), NAMESPACE);
    let brs_lp = ListParams::default()
        .fields(format!("metadata.name={}", associated_bottlerocketshadow_name).as_str());
    let brs_store = reflector::store::Writer::<BottlerocketShadow>::default();
    let brs_reader = brs_store.as_reader();
    let brs_reflector = reflector::reflector(brs_store, watcher(brss, brs_lp));
    let brs_drainer = try_flatten_touched(brs_reflector)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_brs| {
            event!(Level::DEBUG, "Processed event for BottlerocketShadows");
            futures::future::ready(())
        });

    // Generate reflector to watch and cache Nodes
    let node_lp =
        ListParams::default().fields(format!("metadata.name={}", associated_node_name).as_str());
    let nodes: Api<Node> = Api::all(k8s_client.clone());
    let nodes_store = reflector::store::Writer::<Node>::default();
    let node_reader = nodes_store.as_reader();
    let node_reflector = reflector::reflector(nodes_store, watcher(nodes, node_lp));
    let node_drainer = try_flatten_touched(node_reflector)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_node| {
            event!(Level::DEBUG, "Processed event for node");
            futures::future::ready(())
        });

    let mut agent = BrupopAgent::new(
        k8s_client.clone(),
        apiserver_client,
        brs_reader,
        node_reader,
        associated_node_name,
        associated_bottlerocketshadow_name,
    );

    let agent_runner = agent.run();

    tokio::select! {
        _ = brs_drainer => {
            event!(Level::ERROR, "Processed event for brs");
            return Err(error::Error::KubernetesWatcherFailed {object: "brs".to_string()});
        },
        _ = node_drainer => {
            event!(Level::ERROR, "Processed event for node");
            return Err(error::Error::KubernetesWatcherFailed {object: "node".to_string()});
        },
        res = agent_runner => {
            event!(Level::ERROR, "Agent runner exited");
            res.context(error::AgentError)?
        },
    };
    Ok(())
}

/// Initializes global tracing and telemetry state for the agent.
pub fn init_telemetry() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdio_formatting_layer = BunyanFormattingLayer::new(AGENT.into(), std::io::stdout);
    let subscriber = Registry::default()
        .with(env_filter)
        .with(JsonStorageLayer)
        .with(stdio_formatting_layer);
    tracing::subscriber::set_global_default(subscriber).context(error::TracingConfiguration)?;

    Ok(())
}
