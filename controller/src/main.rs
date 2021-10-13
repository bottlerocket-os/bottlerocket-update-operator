use controller::error::{self, Result};
use models::constants::{CONTROLLER, NAMESPACE};
use models::node::{
    BottlerocketNode, BottlerocketNodeClient, BottlerocketNodeState, K8SBottlerocketNodeClient,
};

use futures::stream::StreamExt;
use kube::api::{Api, ListParams};
use kube::core::ResourceExt;
use kube::runtime::reflector::{self, Store};
use kube::runtime::utils::try_flatten_touched;
use kube::runtime::watcher::watcher;
use opentelemetry::sdk::propagation::TraceContextPropagator;
use snafu::ResultExt;
use std::collections::BTreeMap;
use tokio::time::{sleep, Duration};
use tracing::{event, Level};
use tracing_bunyan_formatter::{BunyanFormattingLayer, JsonStorageLayer};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

// Defines the length time after which the controller will take actions.
const ACTION_INTERVAL: Duration = Duration::from_secs(2);

struct BrupopController<T: BottlerocketNodeClient> {
    node_client: T,
    brn_reader: Store<BottlerocketNode>,
}

impl<T: BottlerocketNodeClient> BrupopController<T> {
    fn new(node_client: T, brn_reader: Store<BottlerocketNode>) -> Self {
        BrupopController {
            node_client,
            brn_reader,
        }
    }

    /// Returns a list of all `BottlerocketNode` objects in the cluster.
    fn all_nodes(&self) -> Vec<BottlerocketNode> {
        self.brn_reader.state()
    }

    /// Returns the set of BottlerocketNode objects which is currently being acted upon.
    ///
    /// Nodes are being acted upon if they are not in the `WaitingForUpdate` state, or if their desired state does
    /// not match their current state.
    fn active_node_set(&self) -> BTreeMap<String, BottlerocketNode> {
        self.all_nodes()
            .iter()
            .filter(|brn| {
                brn.status.as_ref().map_or(false, |brn_status| {
                    brn_status.current_state != BottlerocketNodeState::WaitingForUpdate
                        || brn_status.current_state != brn.spec.state
                })
            })
            // kube-rs doesn't implement Ord or Hash on ObjectMeta, so we store these in a map indexed by name.
            // (which are unique within a namespace). `name()` is guaranteed not to panic, as these nodes are all populated
            // by our `reflector`.
            .map(|brn| (brn.name(), brn.clone()))
            .collect()
    }

    /// Determines next actions for a BottlerocketNode.
    async fn progress_node(&self, node: BottlerocketNode) -> Result<()> {
        event!(
            Level::TRACE,
            ?node,
            "Attempting to progress BottlerocketNode."
        );
        let node_status = node
            .status
            .as_ref()
            .ok_or(error::Error::NodeWithoutStatus {
                node_name: node.name().to_string(),
            })?;

        if node_status.current_state == node.spec.state {
            // If the node has reached its desired status, it is ready to move on.
            let desired_spec = node
                .determine_next_spec()
                .context(error::NodeSpecCannotBeDetermined)?;

            event!(
                Level::TRACE,
                ?node,
                ?desired_spec,
                "BottlerocketNode has reached desired status. Modifying spec."
            );

            self.node_client
                .update_node_spec(
                    &node.selector().context(error::NodeSelectorCreation)?,
                    &desired_spec,
                )
                .await
                .context(error::UpdateNodeSpec)
        } else {
            // Otherwise, we need to ensure that the node is making progress in a timely fashion.

            // TODO(seankell) Timeout handling will be added in a future PR.
            Ok(())
        }
    }

    /// This function searches all `BottlerocketNode`s for those which can be transitioned to a new state.
    /// The state transition is then attempted. If successful, this node should be detected as part of the active
    /// set during the next iteration of the controller's event loop.
    async fn search_for_node_to_update(&self) -> Option<BottlerocketNode> {
        for brn in self.all_nodes() {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = brn.determine_next_spec();
            if let Ok(spec) = next_spec {
                if spec != brn.spec {
                    match self.progress_node(brn.clone()).await {
                        Ok(_) => return Some(brn),
                        Err(err) => {
                            event!(Level::ERROR, error = %err, node = ?brn, "Failed to progress node.");
                            continue;
                        }
                    }
                }
            } else {
                continue;
            }
        }
        None
    }

    /// Runs the event loop for the Brupop controller.
    ///
    /// Because the controller wants to gate the number of simultaneously updating nodes, we can't allow the update state machine
    /// of each individual bottlerocket node to run concurrently and in an event-driven fashion. Instead, we will keep an updated
    /// store of `BottlerocketNode` objects based on cluster events, and then periodically make scheduling decisions based on that
    /// store.
    ///
    /// The controller is designed to run on a single node in the cluster and rely on the scheduler to ensure there is always one
    /// running; however, it could be expanded using leader-election and multiple nodes if the scheduler proves to be problematic.
    async fn run(&self) -> Result<()> {
        // On every iteration of the event loop, we reconstruct the state of the controller and determine its
        // next actions. This is to ensure that the operator would behave consistently even if suddenly restarted.
        loop {
            // Brupop typically only operates on a single node at a time. Here we find the set of nodes which is currently undergoing
            // change, to ensure that errors resulting in multiple nodes changing state simultaneously is not unrecoverable.
            let active_set = self.active_node_set();
            event!(Level::TRACE, ?active_set, "Found active set of nodes.");

            if active_set.is_empty() {
                // If there's nothing to operate on, check to see if any other nodes are ready for action.
                // TODO Move to a subroutine create next active node.
                let new_active_node = self.search_for_node_to_update().await;
                event!(Level::TRACE, ?new_active_node, "Found new active node.");
                if let Some(brn) = new_active_node {
                    event!(Level::INFO, name = %brn.name(), "Began updating new node.")
                }
            } else {
                let mut nodes: Vec<BottlerocketNode> = active_set.into_values().collect();
                let mut results: Vec<Result<()>> = vec![];
                for brn in nodes.drain(..) {
                    let result = self.progress_node(brn).await;
                    event!(Level::INFO, progress = ?result, "Attempted to progress active node.");
                    results.push(result);
                }

                // TODO log results
                // TODO state transitions for stuck nodes
            }

            // Sleep until it's time to check for more action.
            sleep(ACTION_INTERVAL).await;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry()?;

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    let brns = Api::<BottlerocketNode>::namespaced(k8s_client.clone(), NAMESPACE);
    let brn_store = reflector::store::Writer::<BottlerocketNode>::default();
    let brn_reader = brn_store.as_reader();

    let node_client = K8SBottlerocketNodeClient::new(k8s_client.clone());

    let controller = BrupopController::new(node_client, brn_reader);
    let controller_runner = controller.run();

    // Setup and run a reflector, ensuring that `BottlerocketNode` updates are reflected to the controller.
    let brn_reflector = reflector::reflector(brn_store, watcher(brns, ListParams::default()));
    let drainer = try_flatten_touched(brn_reflector)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|brn| {
            event!(
                Level::DEBUG,
                brn_name = %brn.name(),
                "Processed a k8s event for a BottlerocketNode object."
            );
            futures::future::ready(())
        });

    tokio::select! {
        _ = drainer => {
            event!(Level::ERROR, "reflector drained");
        },
        _ = controller_runner => {
        event!(Level::ERROR, "controller exited");
        },
    };
    Ok(())
}

fn init_telemetry() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new("trace"));
    let stdio_formatting_layer = BunyanFormattingLayer::new(CONTROLLER.into(), std::io::stdout);
    let subscriber = Registry::default()
        .with(env_filter)
        .with(JsonStorageLayer)
        .with(stdio_formatting_layer);
    tracing::subscriber::set_global_default(subscriber).context(error::TracingConfiguration)?;

    Ok(())
}
