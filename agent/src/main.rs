use agent::agentclient::BrupopAgent;
use apiserver::client::K8SAPIServerClient;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use kube::api::ListParams;
use kube::runtime::reflector;
use kube::runtime::watcher::watcher;
use kube::runtime::WatchStreamExt;
use kube::Api;
use models::agent::{AGENT_TOKEN_PATH, TOKEN_PROJECTION_MOUNT_PATH};
use models::constants::NAMESPACE;

use models::node::{brs_name_from_node_name, BottlerocketShadow};

use opentelemetry::sdk::propagation::TraceContextPropagator;
use snafu::{OptionExt, ResultExt};
use tracing::{event, Level};
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter, Registry};

use std::env;
use std::fs;
use std::path::Path;

const TERMINATION_LOG: &str = "/dev/termination-log";

/// The module-wide result type.
type Result<T> = std::result::Result<T, agent_error::Error>;

#[tokio::main]
async fn main() {
    let termination_log =
        env::var("TERMINATION_LOG").unwrap_or_else(|_| TERMINATION_LOG.to_string());

    if let Err(error) = run_agent().await {
        fs::write(&termination_log, format!("{}", error))
            .expect("Could not write k8s termination log.");
    }
}

async fn run_agent() -> Result<()> {
    init_telemetry()?;

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(agent_error::ClientCreateSnafu)?;

    // Configure our brupop apiserver client to use the auth token mounted to our Pod.
    let token_path = Path::new(TOKEN_PROJECTION_MOUNT_PATH).join(AGENT_TOKEN_PATH);
    let token_path = token_path.to_str().context(agent_error::AssertionSnafu {
        message: "Token path (defined in models/agent.rs) is not valid unicode.",
    })?;
    let apiserver_client = K8SAPIServerClient::new(token_path.to_string());

    // Get node and BottlerocketShadow names
    let associated_node_name = env::var("MY_NODE_NAME").context(agent_error::GetNodeNameSnafu)?;
    let associated_bottlerocketshadow_name = brs_name_from_node_name(&associated_node_name);

    // Generate reflector to watch and cache BottlerocketShadow
    let brss = Api::<BottlerocketShadow>::namespaced(k8s_client.clone(), NAMESPACE);
    let brs_lp = ListParams::default()
        .fields(format!("metadata.name={}", associated_bottlerocketshadow_name).as_str());
    let brs_store = reflector::store::Writer::<BottlerocketShadow>::default();
    let brs_reader = brs_store.as_reader();
    let brs_reflector = reflector::reflector(brs_store, watcher(brss, brs_lp));
    let brs_drainer = brs_reflector
        .touched_objects()
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
    let node_drainer = node_reflector
        .touched_objects()
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_node| {
            event!(Level::DEBUG, "Processed event for node");
            futures::future::ready(())
        });

    let agent = BrupopAgent::new(
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
            return Err(agent_error::Error::KubernetesWatcherFailed {object: "brs".to_string()});
        },
        _ = node_drainer => {
            event!(Level::ERROR, "Processed event for node");
            return Err(agent_error::Error::KubernetesWatcherFailed {object: "node".to_string()});
        },
        res = agent_runner => {
            event!(Level::ERROR, "Agent runner exited");
            res.context(agent_error::AgentSnafu)?
        },
    };
    Ok(())
}

/// Initializes global tracing and telemetry state for the agent.
pub fn init_telemetry() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdio_formatting_layer = fmt::layer().pretty();
    let subscriber = Registry::default()
        .with(env_filter)
        .with(stdio_formatting_layer);
    tracing::subscriber::set_global_default(subscriber)
        .context(agent_error::TracingConfigurationSnafu)?;

    Ok(())
}

pub mod agent_error {
    use agent::agentclient::agentclient_error;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Error running agent server: '{}'", source))]
        AgentError { source: agentclient_error::Error },

        // The assertion type lets us return a Result in cases where we would otherwise use `unwrap()` on results that
        // we know cannot be Err. This lets us bubble up to our error handler which writes to the termination log.
        #[snafu(display("Agent failed due to internal assertion issue: '{}'", message))]
        Assertion { message: String },

        #[snafu(display("Unable to create client: '{}'", source))]
        ClientCreate { source: kube::Error },

        #[snafu(display("Unable to get associated node name: {}", source))]
        GetNodeName { source: std::env::VarError },

        #[snafu(display("The Kubernetes WATCH on {} objects has failed.", object))]
        KubernetesWatcherFailed { object: String },

        #[snafu(display("Error configuring tracing: '{}'", source))]
        TracingConfiguration {
            source: tracing::subscriber::SetGlobalDefaultError,
        },
    }
}
