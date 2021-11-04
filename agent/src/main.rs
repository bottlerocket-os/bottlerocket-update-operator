use agent::agentclient::BrupopAgent;
use agent::error::{self, Result};
use apiserver::client::K8SAPIServerClient;
use k8s_openapi::api::core::v1::Node;
use kube::Api;
use models::agent::{AGENT_TOKEN_PATH, TOKEN_PROJECTION_MOUNT_PATH};

use futures::StreamExt;
use kube::api::ListParams;
use kube::runtime::reflector;
use kube::runtime::utils::try_flatten_touched;
use kube::runtime::watcher::watcher;
use models::constants::NAMESPACE;

use models::node::BottlerocketNode;

use snafu::{OptionExt, ResultExt};

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
    env_logger::init();

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    // Configure our brupop apiserver client to use the auth token mounted to our Pod.
    let token_path = Path::new(TOKEN_PROJECTION_MOUNT_PATH).join(AGENT_TOKEN_PATH);
    let token_path = token_path.to_str().context(error::Assertion {
        message: "Token path (defined in models/agent.rs) is not valid unicode.",
    })?;
    let apiserver_client = K8SAPIServerClient::new(token_path.to_string());

    // Get node and bottlerocketnode names
    let associated_node_name = env::var("MY_NODE_NAME").context(error::GetNodeName)?;
    let associated_bottlerocketnode_name = format!("brn-{}", associated_node_name);

    // Generate reflector to watch and cache BottlerocketNodes
    let brns = Api::<BottlerocketNode>::namespaced(k8s_client.clone(), NAMESPACE);
    let brn_lp = ListParams::default()
        .fields(format!("metadata.name={}", associated_bottlerocketnode_name).as_str());
    let brn_store = reflector::store::Writer::<BottlerocketNode>::default();
    let brn_reader = brn_store.as_reader();
    let brn_reflector = reflector::reflector(brn_store, watcher(brns, brn_lp));
    let brn_drainer = try_flatten_touched(brn_reflector)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_brn| {
            // TODO: utilize tracing event
            log::debug!("Processed event for BottlerocketNodes");
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
            // TODO: utilize tracing event
            log::debug!("Processed event for node");
            futures::future::ready(())
        });

    let mut agent = BrupopAgent::new(
        k8s_client.clone(),
        apiserver_client,
        brn_reader,
        node_reader,
        associated_node_name,
        associated_bottlerocketnode_name,
    );

    let agent_runner = agent.run();

    tokio::select! {
        _ = brn_drainer => {
            log::error!("Processed event for brn");
        },
        _ = node_drainer => {
            log::error!("Processed event for node");
        },
        _ = agent_runner => {
            log::error!("runner exited");
        },
    };
    Ok(())
}
