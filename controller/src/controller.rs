use super::{
    metrics::{BrupopControllerMetrics, BrupopHostsData},
    statemachine::determine_next_node_spec,
};
use models::constants::{BRUPOP_INTERFACE_VERSION, LABEL_BRUPOP_INTERFACE_NAME, NAMESPACE};
use models::node::{
    brs_name_from_node_name, BottlerocketShadow, BottlerocketShadowClient, BottlerocketShadowState,
    Selector,
};

use chrono::{Duration as chrono_duration, NaiveTime, Utc};
use k8s_openapi::api::core::v1::Node;
use kube::api::DeleteParams;
use kube::runtime::reflector::Store;
use kube::Api;
use kube::ResourceExt;
use opentelemetry::global;
use snafu::ResultExt;
use std::collections::{BTreeMap, HashMap};
use std::env;
use tokio::time::{sleep, Duration};
use tracing::{event, instrument, Level};

// Defines the length time after which the controller will take actions.
const ACTION_INTERVAL: Duration = Duration::from_secs(2);

// Defines environment variable name used to fetch max concurrent update number.
const MAX_CONCURRENT_UPDATE_ENV_VAR: &str = "MAX_CONCURRENT_UPDATE";

// Defines the update time window related env variable names
const UPDATE_WINDOW_START_ENV_VAR: &str = "UPDATE_WINDOW_START";
const UPDATE_WINDOW_STOP_ENV_VAR: &str = "UPDATE_WINDOW_STOP";
const UPDATE_WINDOW_BUFFER: i64 = 6;

/// The module-wide result type.
type Result<T> = std::result::Result<T, controllerclient_error::Error>;

/// The BrupopController orchestrates updates across a cluster of Bottlerocket nodes.
pub struct BrupopController<T: BottlerocketShadowClient> {
    k8s_client: kube::client::Client,
    node_client: T,
    brs_reader: Store<BottlerocketShadow>,
    node_reader: Store<Node>,
    metrics: BrupopControllerMetrics,
}

impl<T: BottlerocketShadowClient> BrupopController<T> {
    pub fn new(
        k8s_client: kube::client::Client,
        node_client: T,
        brs_reader: Store<BottlerocketShadow>,
        node_reader: Store<Node>,
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

        sort_shadows(&mut shadows, &get_associated_bottlerocketshadow_name()?);
        for brs in shadows.drain(..) {
            // If we determine that the spec should change, this node is a candidate to begin updating.
            let next_spec = determine_next_node_spec(&brs);
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

        for brs in self.all_brss() {
            if let Some(brs_status) = brs.status {
                let current_version = brs_status.current_version().to_string();
                let current_state = brs_status.current_state;

                *hosts_version_count_map.entry(current_version).or_default() += 1;
                *hosts_state_count_map
                    .entry(serde_plain::to_string(&current_state).context(
                        controllerclient_error::AssertionSnafu {
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
                    Api::namespaced(self.k8s_client.clone(), NAMESPACE);

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
            let active_set = self.active_brs_set();
            let active_set_size = active_set.len();
            event!(Level::TRACE, ?active_set, "Found active set of nodes.");

            // update time window: users can specify a update time window to operate Bottlerocket nodes update. If current time isn't within the time window,
            // controller shouldn't have any action on it.
            match within_time_window()? {
                // If controller has already started a node update but the update time window stops,
                // controller should respect it, continue to complete the update, and stop actions on remaining nodes.
                // This logic will cooperate with 6 mins buffer strategy.
                false => {
                    // try to find out if any nodes are being updated. If yes, controller
                    // will complete them and pause actions on other waitingForUpdate nodes.
                    if !active_set.is_empty() {
                        self.progress_active_set(active_set).await?;
                    } else {
                        sleep(ACTION_INTERVAL).await;
                        continue;
                    }
                }
                true => {
                    if !active_set.is_empty() {
                        self.progress_active_set(active_set).await?;
                    }

                    // Bring one more node each time if the active nodes size is less than MAX_CONCURRENT_UPDATE setting.
                    if active_set_size < get_max_concurrent_update()? {
                        // If there's nothing to operate on, check to see if any other nodes are ready for action.
                        let new_active_node = self.find_and_update_ready_brs().await?;
                        if let Some(brs) = new_active_node {
                            event!(Level::INFO, name = %brs.name_any(), "Began updating new node.")
                        }
                    }
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
        }
    }
}

// Get node and BottlerocketShadow names
#[instrument]
fn get_associated_bottlerocketshadow_name() -> Result<String> {
    let associated_node_name =
        env::var("MY_NODE_NAME").context(controllerclient_error::MissingEnvVariableSnafu {
            variable: "MY_NODE_NAME".to_string(),
        })?;
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
    let max_concurrent_update = env::var(MAX_CONCURRENT_UPDATE_ENV_VAR)
        .context(controllerclient_error::MissingEnvVariableSnafu {
            variable: MAX_CONCURRENT_UPDATE_ENV_VAR.to_string(),
        })?
        .to_lowercase();

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

#[instrument(skip())]
fn within_time_window() -> Result<bool> {
    let update_window_start_env = env::var(UPDATE_WINDOW_START_ENV_VAR).context(
        controllerclient_error::MissingEnvVariableSnafu {
            variable: UPDATE_WINDOW_START_ENV_VAR.to_string(),
        },
    )?;

    let update_window_stop_env = env::var(UPDATE_WINDOW_STOP_ENV_VAR).context(
        controllerclient_error::MissingEnvVariableSnafu {
            variable: UPDATE_WINDOW_STOP_ENV_VAR.to_string(),
        },
    )?;

    let current_time = Utc::now().time();
    let update_window_start = NaiveTime::parse_from_str(&update_window_start_env, "%H:%M:%S")
        .context(controllerclient_error::ConvertToNativeTimeSnafu {
            date: update_window_start_env,
        })?;
    let update_window_stop = NaiveTime::parse_from_str(&update_window_stop_env, "%H:%M:%S")
        .context(controllerclient_error::ConvertToNativeTimeSnafu {
            date: update_window_stop_env,
        })?;

    // Due to the situation that controller has already started a node update but the time window will stop,
    // we design controller to stop updating any new nodes 6 mins (the time brupop spends to update a node)
    // before update window stop time except finishing remaining update circle. Therefore, when update window stops,
    // no nodes are on "in-process" status.
    let update_window_stop_with_buffer =
        update_window_stop - chrono_duration::minutes(UPDATE_WINDOW_BUFFER);

    event!(
        Level::INFO,
        "Calculating if current time is within update time window."
    );
    Ok(update_time_window_calculator(
        &update_window_start,
        &update_window_stop_with_buffer,
        &current_time,
    ))
}

fn update_time_window_calculator(
    update_window_start: &NaiveTime,
    update_window_stop: &NaiveTime,
    current_time: &NaiveTime,
) -> bool {
    // If update window start time is later than update window stop time, we'll assume a cross day period.
    // For example, start time 11pm is later than end time 2am, so brupop will recognize it as 11pm - 2 am (next day) slot.
    if update_window_start > update_window_stop {
        let is_within_time_window =
            (current_time >= update_window_start) || (current_time < update_window_stop);
        return is_within_time_window;
    } else {
        let is_within_time_window =
            (current_time >= update_window_start) && (current_time < update_window_stop);
        return is_within_time_window;
    }
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
    #[test]
    fn test_time_window_calculator() {
        let test_cases = vec![
            (
                btreemap! {
                        "update_window_start" =>
                        NaiveTime::parse_from_str("9:0:0", "%H:%M:%S").unwrap(),
                        "update_window_stop"=>
                        NaiveTime::parse_from_str("18:0:0", "%H:%M:%S").unwrap(),
                        "current_time"=>
                        NaiveTime::parse_from_str("11:0:0", "%H:%M:%S").unwrap(),
                },
                true,
            ),
            (
                btreemap! {
                        "update_window_start" =>
                        NaiveTime::parse_from_str("9:0:0", "%H:%M:%S").unwrap(),
                        "update_window_stop"=>
                        NaiveTime::parse_from_str("18:0:0", "%H:%M:%S").unwrap(),
                        "current_time"=>
                        NaiveTime::parse_from_str("23:0:0", "%H:%M:%S").unwrap(),
                },
                false,
            ),
            (
                btreemap! {
                        "update_window_start" =>
                        NaiveTime::parse_from_str("23:0:0", "%H:%M:%S").unwrap(),
                        "update_window_stop"=>
                        NaiveTime::parse_from_str("5:0:0", "%H:%M:%S").unwrap(),
                        "current_time"=>
                        NaiveTime::parse_from_str("3:0:0", "%H:%M:%S").unwrap(),
                },
                true,
            ),
            (
                btreemap! {
                        "update_window_start" =>
                        NaiveTime::parse_from_str("23:0:0", "%H:%M:%S").unwrap(),
                        "update_window_stop"=>
                        NaiveTime::parse_from_str("5:0:0", "%H:%M:%S").unwrap(),
                        "current_time"=>
                        NaiveTime::parse_from_str("21:0:0", "%H:%M:%S").unwrap(),
                },
                false,
            ),
            (
                btreemap! {
                        "update_window_start" =>
                        NaiveTime::parse_from_str("0:0:0", "%H:%M:%S").unwrap(),
                        "update_window_stop"=>
                        NaiveTime::parse_from_str("0:0:0", "%H:%M:%S").unwrap(),
                        "current_time"=>
                        NaiveTime::parse_from_str("21:0:0", "%H:%M:%S").unwrap(),
                },
                false,
            ),
        ];

        for (times, is_within_time_window) in test_cases {
            assert_eq!(
                update_time_window_calculator(
                    times.get("update_window_start").unwrap(),
                    times.get("update_window_stop").unwrap(),
                    times.get("current_time").unwrap(),
                ),
                is_within_time_window
            );
        }
    }
}

pub mod controllerclient_error {
    use crate::controller::MAX_CONCURRENT_UPDATE_ENV_VAR;
    use models::node::BottlerocketShadowClientError;
    use models::node::BottlerocketShadowError;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Controller failed due to {}: '{}'", msg, source))]
        Assertion {
            msg: String,
            source: serde_plain::Error,
        },

        #[snafu(display("Unable convert {} to Native Time data type due to {}", date, source))]
        ConvertToNativeTime {
            date: String,
            source: chrono::ParseError,
        },

        #[snafu(display("Failed to delete node via kubernetes API: '{}'", source))]
        DeleteNode { source: kube::Error },

        #[snafu(display("Unable to get host controller pod node name: {}", source))]
        GetNodeName { source: std::env::VarError },

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
        #[snafu(display("Unable to get Node UID because of missing Node `UID` value"))]
        MissingNodeUid {},
    }
}
