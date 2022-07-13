use super::{
    metrics::{BrupopControllerMetrics, BrupopHostsData},
    statemachine::determine_next_node_spec,
};
use models::node::{
    brs_name_from_node_name, BottlerocketShadow, BottlerocketShadowClient, BottlerocketShadowState,
    Selector,
};

use kube::runtime::reflector::Store;
use kube::ResourceExt;
use opentelemetry::global;
use snafu::ResultExt;
use std::collections::{BTreeMap, HashMap};
use std::env;
use tokio::time::{sleep, Duration};
use tracing::{event, instrument, Level};

// Defines the length time after which the controller will take actions.
const ACTION_INTERVAL: Duration = Duration::from_secs(2);

/// The module-wide result type.
type Result<T> = std::result::Result<T, controllerclient_error::Error>;

/// The BrupopController orchestrates updates across a cluster of Bottlerocket nodes.
pub struct BrupopController<T: BottlerocketShadowClient> {
    node_client: T,
    brs_reader: Store<BottlerocketShadow>,
    metrics: BrupopControllerMetrics,
}

impl<T: BottlerocketShadowClient> BrupopController<T> {
    pub fn new(node_client: T, brs_reader: Store<BottlerocketShadow>) -> Self {
        // Creates brupop-controller meter via the configured
        // GlobalMeterProvider which is setup in PrometheusExporter
        let meter = global::meter("brupop-controller");
        let metrics = BrupopControllerMetrics::new(meter);
        BrupopController {
            node_client,
            brs_reader,
            metrics,
        }
    }

    /// Returns a list of all `BottlerocketShadow` objects in the cluster.
    fn all_nodes(&self) -> Vec<BottlerocketShadow> {
        self.brs_reader
            .state()
            .iter()
            .map(|arc_brs| (**arc_brs).clone())
            .collect()
    }

    /// Returns the set of BottlerocketShadow objects which is currently being acted upon.
    ///
    /// Nodes are being acted upon if they are not in the `WaitingForUpdate` state, or if their desired state does
    /// not match their current state.
    #[instrument(skip(self))]
    fn active_node_set(&self) -> BTreeMap<String, BottlerocketShadow> {
        self.all_nodes()
            .iter()
            .filter(|brs| {
                brs.status.as_ref().map_or(false, |brs_status| {
                    brs_status.current_state != BottlerocketShadowState::Idle
                        || !brs.has_reached_desired_state()
                })
            })
            // kube-rs doesn't implement Ord or Hash on ObjectMeta, so we store these in a map indexed by name.
            // (which are unique within a namespace). `name()` is guaranteed not to panic, as these nodes are all populated
            // by our `reflector`.
            .map(|brs| (brs.name(), brs.clone()))
            .collect()
    }

    /// Determines next actions for a BottlerocketShadow and attempts to execute them.
    ///
    /// This could include modifying the `spec` of a brs to indicate a new desired state, or handling timeouts.
    #[instrument(skip(self), err)]
    async fn progress_node(&self, node: BottlerocketShadow) -> Result<()> {
        if node.has_reached_desired_state() || node.has_crashed() {
            // Emit metrics to show the existing status
            self.emit_metrics()?;

            let desired_spec = determine_next_node_spec(&node);

            event!(
                Level::INFO,
                ?desired_spec,
                "BottlerocketShadow has reached desired status. Modifying spec."
            );

            self.node_client
                .update_node_spec(
                    &node
                        .selector()
                        .context(controllerclient_error::NodeSelectorCreation)?,
                    &desired_spec,
                )
                .await
                .context(controllerclient_error::UpdateNodeSpec)
        } else {
            // Otherwise, we need to ensure that the node is making progress in a timely fashion.

            // TODO(seankell) Timeout handling will be added in a future PR.
            Ok(())
        }
    }

    /// This function searches all `BottlerocketShadow`s for those which can be transitioned to a new state.
    /// The state transition is then attempted. If successful, this node should be detected as part of the active
    /// set during the next iteration of the controller's event loop.
    #[instrument(skip(self))]
    async fn find_and_update_ready_node(&self) -> Result<Option<BottlerocketShadow>> {
        let mut shadows: Vec<BottlerocketShadow> = self.all_nodes();

        sort_shadows(&mut shadows, &get_associated_bottlerocketshadow_name()?);
        for brs in shadows.drain(..) {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = determine_next_node_spec(&brs);
            if next_spec != brs.spec {
                match self.progress_node(brs.clone()).await {
                    Ok(_) => return Ok(Some(brs)),
                    Err(_) => {
                        // Errors connecting to the k8s API are ignored (and also logged by `progress_node()`).
                        // We'll just move on and try a different node.
                        continue;
                    }
                }
            }
        }
        Ok(None)
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

        for brs in self.all_nodes() {
            if let Some(brs_status) = brs.status {
                let current_version = brs_status.current_version().to_string();
                let current_state = brs_status.current_state;

                *hosts_version_count_map.entry(current_version).or_default() += 1;
                *hosts_state_count_map
                    .entry(serde_plain::to_string(&current_state).context(
                        controllerclient_error::Assertion {
                            msg: "unable to parse current_state".to_string(),
                        },
                    )?)
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
    /// Instead, we will keep an updated store of `BottlerocketShadow` objects based on cluster events, and then periodically make
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
                let mut nodes: Vec<BottlerocketShadow> = active_set.into_values().collect();

                for brs in nodes.drain(..) {
                    // Timeouts and errors are logged by instrumentation in `progress_node()`.
                    #[allow(unused_must_use)]
                    {
                        self.progress_node(brs).await;
                    }
                }
            } else {
                // If there's nothing to operate on, check to see if any other nodes are ready for action.
                let new_active_node = self.find_and_update_ready_node().await?;
                if let Some(brs) = new_active_node {
                    event!(Level::INFO, name = %brs.name(), "Began updating new node.")
                }
            }

            // Emit metrics at the end of the loop in case the loop didn't progress any nodes.
            self.emit_metrics()?;

            // Sleep until it's time to check for more action.
            sleep(ACTION_INTERVAL).await;
        }
    }
}

// Get node and BottlerocketShadow names
#[instrument]
fn get_associated_bottlerocketshadow_name() -> Result<String> {
    let associated_node_name =
        env::var("MY_NODE_NAME").context(controllerclient_error::GetNodeName)?;
    let associated_bottlerocketshadow_name = brs_name_from_node_name(&associated_node_name);

    event!(
        Level::INFO,
        ?associated_bottlerocketshadow_name,
        "Found associated bottlerocketshadow name."
    );

    Ok(associated_bottlerocketshadow_name)
}

/// sort shadows list which uses to determine the order of node update
/// logic1: sort shadows by crash count
/// logic2: move the shadow which associated bottlerocketshadow node hosts controller pod to the last
#[instrument(skip())]
fn sort_shadows(shadows: &mut Vec<BottlerocketShadow>, associated_brs_name: &str) {
    // sort shadows by crash count
    shadows.sort_by(|a, b| a.compare_crash_count(b));

    // move the shadow which associated bottlerocketshadow node hosts controller pod to the last
    // Step1: find associated brs node position
    let associated_brs_node_position = shadows
        .iter()
        .position(|brs| brs.metadata.name.as_ref() == Some(&associated_brs_name.to_string()));

    // Step2: move associated brs node to the last
    // if it doesn't find the brs, it means some brss aren't ready and the program should skip sort.
    match associated_brs_node_position {
        Some(position) => {
            let last_brs = shadows[position].clone();
            shadows.remove(position.clone());
            shadows.push(last_brs);
        }
        None => {
            event!(
                Level::INFO,
                "Unable to find associated bottlerocketshadow, skip sort."
            )
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use chrono::{TimeZone, Utc};
    use semver::Version;
    use std::str::FromStr;

    use kube::api::ObjectMeta;

    use models::node::{BottlerocketShadow, BottlerocketShadowState, BottlerocketShadowStatus};

    pub(crate) fn fake_shadow(
        name: String,
        current_version: String,
        target_version: String,
        current_state: BottlerocketShadowState,
    ) -> BottlerocketShadow {
        BottlerocketShadow {
            status: Some(BottlerocketShadowStatus::new(
                Version::from_str(&current_version).unwrap(),
                Version::from_str(&target_version).unwrap(),
                current_state,
                0,
                Some(Utc.ymd(2022, 1, 1).and_hms_milli(0, 0, 1, 444)),
            )),
            metadata: ObjectMeta {
                name: Some(name),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_sort_shadows_find_brs() {
        let mut test_shadows = vec![
            fake_shadow(
                "brs-ip-1.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-2.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-3.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
        ];

        let associated_brs_name = "brs-ip-1.us-west-2.compute.internal";

        let expected_result = vec![
            fake_shadow(
                "brs-ip-2.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-3.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-1.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
        ];

        sort_shadows(&mut test_shadows, associated_brs_name);

        assert_eq!(test_shadows, expected_result);
    }

    /// test when it doesn't find the brs (some brss aren't ready), the program should skip sort.
    #[tokio::test]
    async fn test_sort_shadows_not_find_brs() {
        let mut test_shadows = vec![
            fake_shadow(
                "brs-ip-17.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-123.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-321.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
        ];

        let associated_brs_name = "brs-ip-5.us-west-2.compute.internal";

        let expected_result = vec![
            fake_shadow(
                "brs-ip-17.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-123.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
            fake_shadow(
                "brs-ip-321.us-west-2.compute.internal".to_string(),
                "1.8.0".to_string(),
                "1.5.0".to_string(),
                BottlerocketShadowState::Idle,
            ),
        ];

        sort_shadows(&mut test_shadows, associated_brs_name);

        assert_eq!(test_shadows, expected_result);
    }
}
pub mod controllerclient_error {
    use models::node::BottlerocketShadowError;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum Error {
        #[snafu(display("Controller failed due to {}: '{}'", msg, source))]
        Assertion {
            msg: String,
            source: serde_plain::Error,
        },

        #[snafu(display("Unable to get host controller pod node name: {}", source))]
        GetNodeName { source: std::env::VarError },

        #[snafu(display("Failed to update node spec via kubernetes API: '{}'", source))]
        UpdateNodeSpec { source: BottlerocketShadowError },

        #[snafu(display("Could not determine selector for node: '{}'", source))]
        NodeSelectorCreation { source: BottlerocketShadowError },
    }
}
