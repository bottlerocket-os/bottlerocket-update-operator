use models::node::{BottlerocketShadow, BottlerocketShadowSpec, BottlerocketShadowState};

use tracing::instrument;

/// Constructs a `BottlerocketShadowSpec` to assign to a `BottlerocketShadow` resource, assuming the current
/// spec has been successfully achieved.
#[instrument(skip(brs))]
pub fn determine_next_node_spec(brs: &BottlerocketShadow) -> BottlerocketShadowSpec {
    match brs.status.as_ref() {
        // If no status is present, just keep waiting for an update.
        None => BottlerocketShadowSpec::default(),
        // If we've not actualized the current spec, then don't bother computing a new one.
        Some(node_status) if node_status.current_state != brs.spec.state => brs.spec.clone(),
        Some(node_status) => {
            match brs.spec.state {
                BottlerocketShadowState::Idle => {
                    let target_version = node_status.target_version();
                    Some(target_version)
                        .filter(|target_version| &node_status.current_version() != target_version)
                        .map(|target_version| {
                            BottlerocketShadowSpec::new_starting_now(
                                BottlerocketShadowState::StagedUpdate,
                                Some(target_version.clone()),
                            )
                        })
                        .unwrap_or_else(BottlerocketShadowSpec::default)
                }
                BottlerocketShadowState::MonitoringUpdate => {
                    // We're ready to wait for a new update.
                    // For now, we just proceed right away.
                    // TODO implement a monitoring protocol
                    // Customers can:
                    //   * specify a k8s job which checks for success
                    //   * allow a default job to test for success
                    //   * proceed right away
                    BottlerocketShadowSpec::new_starting_now(
                        brs.spec.state.on_success(),
                        brs.spec.version(),
                    )
                }
                // In any other circumstance, we just proceed to the next step.
                _ => BottlerocketShadowSpec::new_starting_now(
                    brs.spec.state.on_success(),
                    brs.spec.version(),
                ),
            }
        }
    }
}
