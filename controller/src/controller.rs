use super::{
    error::{self, Result},
    metrics::{BrupopControllerMetrics, BrupopHostsData},
    statemachine::determine_next_node_spec,
};
use models::node::{BottlerocketNode, BottlerocketNodeClient, BottlerocketNodeState};

use kube::runtime::reflector::Store;
use kube::ResourceExt;
use opentelemetry::global;
use snafu::ResultExt;
use std::collections::{BTreeMap, HashMap};
use tokio::time::{sleep, Duration};
use tracing::{event, instrument, Level};

// Defines the length time after which the controller will take actions.
const ACTION_INTERVAL: Duration = Duration::from_secs(2);

/// The BrupopController orchestrates updates across a cluster of Bottlerocket nodes.
pub struct BrupopController<T: BottlerocketNodeClient> {
    node_client: T,
    brn_reader: Store<BottlerocketNode>,
    metrics: BrupopControllerMetrics,
}

impl<T: BottlerocketNodeClient> BrupopController<T> {
    pub fn new(node_client: T, brn_reader: Store<BottlerocketNode>) -> Self {
        // Creates brupop-controller meter via the configured
        // GlobalMeterProvider which is setup in PrometheusExporter
        let meter = global::meter("brupop-controller");
        let metrics = BrupopControllerMetrics::new(meter);
        BrupopController {
            node_client,
            brn_reader,
            metrics,
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
    #[instrument(skip(self))]
    fn active_node_set(&self) -> BTreeMap<String, BottlerocketNode> {
        self.all_nodes()
            .iter()
            .filter(|brn| {
                brn.status.as_ref().map_or(false, |brn_status| {
                    brn_status.current_state != BottlerocketNodeState::Idle
                        || !brn.has_reached_desired_state()
                })
            })
            // kube-rs doesn't implement Ord or Hash on ObjectMeta, so we store these in a map indexed by name.
            // (which are unique within a namespace). `name()` is guaranteed not to panic, as these nodes are all populated
            // by our `reflector`.
            .map(|brn| (brn.name(), brn.clone()))
            .collect()
    }

    /// Determines next actions for a BottlerocketNode and attempts to execute them.
    ///
    /// This could include modifying the `spec` of a brn to indicate a new desired state, or handling timeouts.
    #[instrument(skip(self), err)]
    async fn progress_node(&self, node: BottlerocketNode) -> Result<()> {
        if node.has_reached_desired_state() {
            // Emit metrics to show the existing status
            self.emit_metrics()?;

            let desired_spec = determine_next_node_spec(&node);

            event!(
                Level::INFO,
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
    #[instrument(skip(self))]
    async fn find_and_update_ready_node(&self) -> Option<BottlerocketNode> {
        for brn in self.all_nodes() {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = determine_next_node_spec(&brn);
            if next_spec != brn.spec {
                match self.progress_node(brn.clone()).await {
                    Ok(_) => return Some(brn),
                    Err(_) => {
                        // Errors connecting to the k8s API are ignored (and also logged by `progress_node()`).
                        // We'll just move on and try a different node.
                        continue;
                    }
                }
            }
        }
        None
    }

    #[instrument(skip(self))]
    fn emit_metrics(&self) -> Result<()> {
        let data = self.fetch_data()?;
        self.metrics.emit_metrics(data);
        Ok(())
    }

    /// Fetch the custom resources status for all resources
    /// to gather the information on hosts's bottlerocket version
    /// and brupop state.
    #[instrument(skip(self))]
    fn fetch_data(&self) -> Result<BrupopHostsData> {
        let mut hosts_version_count_map = HashMap::new();
        let mut hosts_state_count_map = HashMap::new();

        for brn in self.all_nodes() {
            if let Some(brn_status) = brn.status {
                let current_version = brn_status.current_version().to_string();
                let current_state = brn_status.current_state;

                *hosts_version_count_map.entry(current_version).or_default() += 1;
                *hosts_state_count_map
                    .entry(serde_plain::to_string(&current_state).context(error::Assertion)?)
                    .or_default() += 1;
            }
        }

        Ok(BrupopHostsData::new(
            hosts_version_count_map,
            hosts_state_count_map,
        ))
    }

    /// Runs the event loop for the Brupop controller.
    ///
    /// Because the controller wants to gate the number of simultaneously updating nodes, we can't allow the update state machine
    /// of each individual bottlerocket node to run concurrently and in an event-driven fashion, as is typically done with controllers.
    /// Instead, we will keep an updated store of `BottlerocketNode` objects based on cluster events, and then periodically make
    /// scheduling decisions based on that store.
    ///
    /// The controller is designed to run on a single node in the cluster and rely on the scheduler to ensure there is always one
    /// running; however, it could be expanded using leader-election and multiple nodes if the scheduler proves to be problematic.
    pub async fn run(&self) -> Result<()> {
        // On every iteration of the event loop, we reconstruct the state of the controller and determine its
        // next actions. This is to ensure that the operator would behave consistently even if suddenly restarted.
        loop {
            // Brupop typically only operates on a single node at a time. Here we find the set of nodes which is currently undergoing
            // change, to ensure that errors resulting in multiple nodes changing state simultaneously is not unrecoverable.
            let active_set = self.active_node_set();
            event!(Level::TRACE, ?active_set, "Found active set of nodes.");

            if !active_set.is_empty() {
                // Try to push forward all active nodes, gathering results along the way.
                let mut nodes: Vec<BottlerocketNode> = active_set.into_values().collect();

                for brn in nodes.drain(..) {
                    // Timeouts and errors are logged by instrumentation in `progress_node()`.
                    #[allow(unused_must_use)]
                    {
                        self.progress_node(brn).await;
                    }
                }
            } else {
                // If there's nothing to operate on, check to see if any other nodes are ready for action.
                let new_active_node = self.find_and_update_ready_node().await;
                if let Some(brn) = new_active_node {
                    event!(Level::INFO, name = %brn.name(), "Began updating new node.")
                }
            }

            // Emit metrics at the end of the loop in case the loop didn't progress any nodes.
            self.emit_metrics()?;

            // Sleep until it's time to check for more action.
            sleep(ACTION_INTERVAL).await;
        }
    }
}
