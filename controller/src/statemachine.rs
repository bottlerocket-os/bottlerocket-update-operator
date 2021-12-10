use models::node::{BottlerocketNode, BottlerocketNodeSpec, BottlerocketNodeState};

use tracing::instrument;

/// Constructs a `BottlerocketNodeSpec` to assign to a `BottlerocketNode` resource, assuming the current
/// spec has been successfully achieved.
#[instrument(skip(brn))]
pub fn determine_next_node_spec(brn: &BottlerocketNode) -> BottlerocketNodeSpec {
    match brn.status.as_ref() {
        // If no status is present, just keep waiting for an update.
        None => BottlerocketNodeSpec::default(),
        // If we've not actualized the current spec, then don't bother computing a new one.
        Some(node_status) if node_status.current_state != brn.spec.state => brn.spec.clone(),
        Some(node_status) => {
            match brn.spec.state {
                BottlerocketNodeState::Idle => {
                    let target_version = node_status.target_version();
                    Some(target_version)
                        .filter(|target_version| &node_status.current_version() != target_version)
                        .map(|target_version| {
                            BottlerocketNodeSpec::new_starting_now(
                                BottlerocketNodeState::StagedUpdate,
                                Some(target_version.clone()),
                            )
                        })
                        .unwrap_or_else(BottlerocketNodeSpec::default)
                }
                BottlerocketNodeState::MonitoringUpdate => {
                    // We're ready to wait for a new update.
                    // For now, we just proceed right away.
                    // TODO implement a monitoring protocol
                    // Customers can:
                    //   * specify a k8s job which checks for success
                    //   * allow a default job to test for success
                    //   * proceed right away
                    BottlerocketNodeSpec::new_starting_now(
                        brn.spec.state.on_success(),
                        brn.spec.version(),
                    )
                }
                // In any other circumstance, we just proceed to the next step.
                _ => BottlerocketNodeSpec::new_starting_now(
                    brn.spec.state.on_success(),
                    brn.spec.version(),
                ),
            }
        }
    }
}
