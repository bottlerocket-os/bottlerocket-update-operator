use crate::metrics;

use super::{
    metrics::BrupopControllerMetrics, scheduler::BrupopCronScheduler,
    statemachine::determine_next_node_spec,
};
use models::constants::{BRUPOP_INTERFACE_VERSION, LABEL_BRUPOP_INTERFACE_NAME};
use models::node::{
    brs_name_from_node_name, BottlerocketShadow, BottlerocketShadowClient, BottlerocketShadowState,
    Selector,
};

use k8s_openapi::api::core::v1::Node;
use kube::api::DeleteParams;
use kube::runtime::reflector::Store;
use kube::Api;
use kube::ResourceExt;
use opentelemetry::global;
use snafu::ResultExt;
use std::collections::BTreeMap;
use std::env;
use tokio::time::{sleep, Duration};

use tracing::{event, instrument, Level};

// Defines the length time after which the controller will take actions.
const ACTION_INTERVAL: Duration = Duration::from_secs(2);

// The interval between control loop polls if no nodes are detected.
const CANNOT_FIND_ANY_NODES_WAIT_INTERVAL: Duration = Duration::from_secs(10);

// Defines environment variable name used to fetch max concurrent update number.
const MAX_CONCURRENT_UPDATE_ENV_VAR: &str = "MAX_CONCURRENT_UPDATE";

/// The module-wide result type.
type Result<T> = std::result::Result<T, controllerclient_error::Error>;

/// The BrupopController orchestrates updates across a cluster of Bottlerocket nodes.
pub struct BrupopController<T: BottlerocketShadowClient> {
    k8s_client: kube::client::Client,
    node_client: T,
    brs_reader: Store<BottlerocketShadow>,
    node_reader: Store<Node>,
    metrics: BrupopControllerMetrics,
    namespace: String,
}

impl<T: BottlerocketShadowClient> BrupopController<T> {
    pub fn new(
        k8s_client: kube::client::Client,
        node_client: T,
        brs_reader: Store<BottlerocketShadow>,
        node_reader: Store<Node>,
        namespace: &str,
    ) -> Self {
        // Creates brupop-controller meter via the configured
        // GlobalMeterProvider which is setup in PrometheusExporter
        let meter = global::meter("brupop-controller");
        let metrics = BrupopControllerMetrics::new(meter);
        BrupopController {
            k8s_client,
            node_client,
            brs_reader,
            node_reader,
            metrics,
            namespace: namespace.to_string(),
        }
    }

    /// Returns a list of all custom definition resource `BottlerocketShadow`/`brs` objects in the cluster.
    fn all_brss(&self) -> Vec<BottlerocketShadow> {
        self.brs_reader
            .state()
            .iter()
            .map(|arc_brs| (**arc_brs).clone())
            .collect()
    }

    /// Returns a list of all bottlerocket nodes in the cluster.
    fn all_nodes(&self) -> Vec<Node> {
        self.node_reader
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
    fn active_brs_set(&self) -> BTreeMap<String, BottlerocketShadow> {
        self.all_brss()
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
            .map(|brs| (brs.name_any(), brs.clone()))
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
                        .context(controllerclient_error::NodeSelectorCreationSnafu)?,
                    &desired_spec,
                )
                .await
                .context(controllerclient_error::UpdateNodeSpecSnafu)
        } else {
            // Otherwise, we need to ensure that the node is making progress in a timely fashion.
            event!(
                Level::TRACE,
                node = ?node.name_any(),
                "Node is still making progress towards its current spec."
            );

            // TODO(seankell) Timeout handling will be added in a future PR.
            Ok(())
        }
    }

    /// This function searches all `BottlerocketShadow`s for those
    /// which can be transitioned from initial state to a new state.
    /// The state transition is then attempted. If successful, this node should be detected as part of the active
    /// set during the next iteration of the controller's event loop.
    #[instrument(skip(self))]
    async fn find_and_update_ready_brs(&self) -> Result<Option<BottlerocketShadow>> {
        let mut shadows: Vec<BottlerocketShadow> = self.all_brss();
        event!(
            Level::TRACE,
            shadows = ?shadows.iter().map(|i| i.name_any()).collect::<Vec<_>>(),
            "Checking shadows for any that are ready to update."
        );

        sort_shadows(&mut shadows, &get_associated_bottlerocketshadow_name()?);
        event!(
            Level::TRACE,
            shadows = ?shadows.iter().map(|i| i.name_any()).collect::<Vec<_>>(),
            "Sorted shadows by update priority."
        );

        for brs in shadows.drain(..) {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = determine_next_node_spec(&brs);
            event!(
                Level::TRACE,
                brs = ?brs.name_any(),
                current_spec = ?brs.spec,
                ?next_spec,
                "Evaluated next spec for node {}", brs.name_any()
            );
            if next_spec != brs.spec && is_initial_state(&brs) {
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
        let metrics_data = metrics::BrupopHostsData::from_shadows(&self.all_brss())
            .context(controllerclient_error::MetricsComputeSnafu)?;
        self.metrics.emit_metrics(metrics_data);
        Ok(())
    }

    #[instrument(skip(self, nodes, brss_name))]
    async fn bottlerocketshadows_cleanup(
        &self,
        nodes: Vec<Node>,
        brss_name: Vec<String>,
    ) -> Result<()> {
        let unlabeled_nodes = find_unlabeled_nodes(nodes);

        for unlabeled_node in unlabeled_nodes {
            let associated_bottlerocketshadow = brs_name_from_node_name(&unlabeled_node);
            if brss_name
                .iter()
                .any(|x| x == &associated_bottlerocketshadow)
            {
                event!(
                    Level::INFO,
                    name = &associated_bottlerocketshadow.as_str(),
                    "Begin deleting brs."
                );

                let bottlerocket_shadows: Api<BottlerocketShadow> =
                    Api::namespaced(self.k8s_client.clone(), &self.namespace);

                bottlerocket_shadows
                    .delete(
                        associated_bottlerocketshadow.as_str(),
                        &DeleteParams::default(),
                    )
                    .await
                    .context(controllerclient_error::DeleteNodeSnafu)?;
            }
        }
        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn progress_active_set(
        &self,
        active_set: BTreeMap<String, BottlerocketShadow>,
    ) -> Result<()> {
        // Try to push forward all active nodes, gathering results along the way.
        let mut nodes: Vec<BottlerocketShadow> = active_set.into_values().collect();

        for brs in nodes.drain(..) {
            // Timeouts and errors are logged by instrumentation in `progress_node()`.
            #[allow(unused_must_use)]
            {
                self.progress_node(brs).await;
            }
        }
        Ok(())
    }

    #[instrument(skip(self))]
    fn nodes_ready_to_update(&self) -> bool {
        self.all_brss().iter().any(|brs| {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = determine_next_node_spec(brs);
            next_spec != brs.spec && is_initial_state(brs)
        })
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
        // generate brupop cron expression schedule
        let scheduler = BrupopCronScheduler::from_environment()
            .context(controllerclient_error::GetCronScheduleSnafu)?;

        // On every iteration of the event loop, we reconstruct the state of the controller and determine its
        // next actions. This is to ensure that the operator would behave consistently even if suddenly restarted.
        loop {
            if self.all_brss().is_empty() {
                event!(
                    Level::INFO,
                    "Nothing to do: The bottlerocket-update-operator is not aware of any BottlerocketShadow objects. \
                    Is the bottlerocket-shadow CRD installed? Are nodes labelled so that the agent is deployed to them? \
                    See the project's README for more information.",
                );
                sleep(CANNOT_FIND_ANY_NODES_WAIT_INTERVAL).await;
                continue;
            }

            let active_set = self.active_brs_set();
            event!(Level::TRACE, active_set = ?active_set.keys().collect::<Vec<_>>(), "Found active set of nodes.");

            // when current is outside of a scheduled maintenance window, controlle should keep updating active nodes
            // if there are any ongoing updates. Otherwise it should sleep until next maintenance window.
            let mut maintenance_window = if active_set.is_empty() {
                // If there are no more active nodes and current is outside of the maintenance window, brupop controller
                // will sleep until next scheduled time.
                scheduler
                    .wait_until_next_maintainence_window()
                    .await
                    .context(controllerclient_error::SleepUntilNextScheduleSnafu)?;
                true
            } else {
                // Any ongoing updates are completed even outside of the maintenance window
                self.progress_active_set(active_set).await?;
                sleep(ACTION_INTERVAL).await;
                false
            };

            while maintenance_window {
                // Brupop typically only operates on a single node at a time. Here we find the set of nodes which is currently undergoing
                // change, to ensure that errors resulting in multiple nodes changing state simultaneously is not unrecoverable.
                let active_set = self.active_brs_set();
                let active_set_size = active_set.len();
                event!(Level::TRACE, active_set = ?active_set.keys().collect::<Vec<_>>(), "Found active set of nodes.");

                if !active_set.is_empty() {
                    self.progress_active_set(active_set).await?;
                }
                // Bring one more node each time if the active nodes size is less than MAX_CONCURRENT_UPDATE setting.
                let max_concurrent_updates = get_max_concurrent_update()?;
                if active_set_size < max_concurrent_updates {
                    event!(
                        Level::TRACE,
                        ?active_set_size,
                        ?max_concurrent_updates,
                        "Searching for more nodes to update."
                    );
                    // If there's nothing to operate on, check to see if any other nodes are ready for action.
                    let new_active_node = self.find_and_update_ready_brs().await?;
                    if let Some(brs) = new_active_node {
                        event!(Level::INFO, name = %brs.name_any(), "Began updating new node.")
                    }
                }

                // Cleanup BRS when the operator is removed from a node
                let brss_name = self
                    .all_brss()
                    .into_iter()
                    .map(|brs| brs.name_any())
                    .collect();
                let nodes = self.all_nodes();
                self.bottlerocketshadows_cleanup(nodes, brss_name).await?;

                // Emit metrics at the end of the loop in case the loop didn't progress any nodes.
                self.emit_metrics()?;

                // Sleep until it's time to check for more action.
                sleep(ACTION_INTERVAL).await;

                // We end the maintenance window if it's unable to find ready node, or the time window has ended.
                maintenance_window =
                    !scheduler.should_discontinue_updates() && self.nodes_ready_to_update();
            }
        }
    }
}

// Get node and BottlerocketShadow names
#[instrument]
fn get_associated_bottlerocketshadow_name() -> Result<String> {
    let associated_node_name = read_env_var("MY_NODE_NAME")?;
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
#[instrument(skip(shadows))]
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
            shadows.remove(position);
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

/// Fetch the environment variable to determine the max concurrent update nodes number.
fn get_max_concurrent_update() -> Result<usize> {
    let max_concurrent_update = read_env_var(MAX_CONCURRENT_UPDATE_ENV_VAR)?.to_lowercase();

    if max_concurrent_update.eq("unlimited") {
        Ok(usize::MAX)
    } else {
        max_concurrent_update
            .parse::<usize>()
            .context(controllerclient_error::MaxConcurrentUpdateParseSnafu)
    }
}

/// Determine if a BottlerocketShadow is in default or None status.
fn is_initial_state(brs: &BottlerocketShadow) -> bool {
    match brs.status.clone() {
        None => true,
        Some(status) => status.current_state == BottlerocketShadowState::default(),
    }
}

#[instrument(skip(nodes))]
fn find_unlabeled_nodes(mut nodes: Vec<Node>) -> Vec<String> {
    let mut unlabeled_nodes: Vec<String> = Vec::new();
    for node in nodes.drain(..) {
        if !node_has_label(&node.clone()) {
            unlabeled_nodes.push(node.name_any());
        }
    }

    unlabeled_nodes
}

#[instrument(skip(node))]
fn node_has_label(node: &Node) -> bool {
    return node.labels().get_key_value(LABEL_BRUPOP_INTERFACE_NAME)
        == Some((
            &LABEL_BRUPOP_INTERFACE_NAME.to_string(),
            &BRUPOP_INTERFACE_VERSION.to_string(),
        ));
}

fn read_env_var(env_var: &str) -> Result<String> {
    env::var(env_var).context(controllerclient_error::MissingEnvVariableSnafu {
        variable: env_var.to_string(),
    })
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use chrono::{TimeZone, Utc};
    use maplit::btreemap;
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
                Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 1).unwrap()),
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

    #[test]
    fn test_get_max_concurrent_update() {
        let variable_value_tuple =
            vec![("unlimited".to_string(), usize::MAX), ("2".to_string(), 2)];

        for (env_val, target_val) in variable_value_tuple {
            env::set_var(MAX_CONCURRENT_UPDATE_ENV_VAR, env_val);
            assert_eq!(get_max_concurrent_update().unwrap(), target_val);
        }
    }

    #[tokio::test]
    #[allow(clippy::bool_assert_comparison)]
    async fn test_node_has_label() {
        let labeled_node = Node {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                labels: Some(btreemap! {
                    LABEL_BRUPOP_INTERFACE_NAME.to_string() => BRUPOP_INTERFACE_VERSION.to_string(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let unlabeled_node = Node {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(node_has_label(&labeled_node));
        assert_eq!(node_has_label(&unlabeled_node), false);
    }
}

pub mod controllerclient_error {
    use crate::controller::MAX_CONCURRENT_UPDATE_ENV_VAR;
    use crate::scheduler::scheduler_error;
    use models::node::BottlerocketShadowClientError;
    use models::node::BottlerocketShadowError;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Failed to delete node via kubernetes API: '{}'", source))]
        DeleteNode { source: kube::Error },

        #[snafu(display("Unable to get host controller pod node name: {}", source))]
        GetNodeName { source: std::env::VarError },

        #[snafu(display("Unable to get cron expression schedule: {}", source))]
        GetCronSchedule { source: scheduler_error::Error },

        #[snafu(display("Failed to update node spec via kubernetes API: '{}'", source))]
        UpdateNodeSpec {
            source: BottlerocketShadowClientError,
        },

        #[snafu(display("Could not determine selector for node: '{}'", source))]
        NodeSelectorCreation { source: BottlerocketShadowError },

        #[snafu(display(
            "Unable to get environment variable '{}' due to : '{}'",
            variable,
            source
        ))]
        MissingEnvVariable {
            source: std::env::VarError,
            variable: String,
        },

        #[snafu(display(
            "Unable to parse environment variable '{}': '{}'",
            MAX_CONCURRENT_UPDATE_ENV_VAR,
            source
        ))]
        MaxConcurrentUpdateParseError { source: std::num::ParseIntError },

        #[snafu(display("Error creating maintenance time: '{}'", source))]
        MaintenanceTimeError { source: scheduler_error::Error },

        #[snafu(display("Failed to compute cluster metrics: '{}'", source))]
        MetricsCompute {
            source: crate::metrics::error::MetricsError,
        },

        #[snafu(display("Unable to find next scheduled time and sleep: '{}'", source))]
        SleepUntilNextSchedule { source: scheduler_error::Error },
    }
}
