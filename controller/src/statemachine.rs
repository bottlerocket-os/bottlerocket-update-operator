use models::node::{
    BottlerocketShadow, BottlerocketShadowSpec, BottlerocketShadowState, BottlerocketShadowStatus,
};

use chrono::Utc;
use tracing::instrument;
use tracing::{event, Level};

const RETRY_MAX_DELAY_IN_MINUTES: i64 = 24 * 60;

/// Constructs a `BottlerocketShadowSpec` to assign to a `BottlerocketShadow` resource, assuming the current
/// spec has been successfully achieved.
#[instrument(skip(brs))]
pub fn determine_next_node_spec(brs: &BottlerocketShadow) -> BottlerocketShadowSpec {
    match brs.status.as_ref() {
        // If no status is present, just keep waiting for an update.
        None => BottlerocketShadowSpec::default(),
        // If we've not actualized the current spec, then don't bother computing a new one.
        Some(node_status) if node_status.current_state != brs.spec.state => {
            if node_status.current_state != BottlerocketShadowState::ErrorReset {
                // Wait for the update to complete
                brs.spec.clone()
            } else {
                event!(Level::INFO, "Discovered that agent had crashed");
                // Agent has crashed
                BottlerocketShadowSpec::new_starting_now(
                    BottlerocketShadowState::Idle,
                    brs.spec.version(),
                )
            }
        }
        Some(node_status) => {
            match brs.spec.state {
                BottlerocketShadowState::Idle => {
                    let target_version = node_status.target_version();
                    if node_status.current_version() != target_version {
                        // Node crashed before but reached time to retry
                        // Or node just start or completed without crashing
                        if node_allowed_to_update(node_status) {
                            BottlerocketShadowSpec::new_starting_now(
                                BottlerocketShadowState::StagedAndPerformedUpdate,
                                Some(target_version),
                            )
                        } else {
                            // Do nothing if not reach the wait time
                            brs.spec.clone()
                        }
                    } else {
                        BottlerocketShadowSpec::default()
                    }
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

/// Returns whether or not an Idle node is allowed to enter an update workflow.
/// This returns false if the node has previously encountered an error and not yet
/// passed its retry timer.
fn node_allowed_to_update(node_status: &BottlerocketShadowStatus) -> bool {
    if let Some(crash_time) = node_status.failure_timestamp().unwrap() {
        let time_gap = (Utc::now() - crash_time).num_minutes();
        exponential_backoff_time_with_upper_limit(
            time_gap,
            node_status.crash_count(),
            RETRY_MAX_DELAY_IN_MINUTES,
        )
    } else {
        // Never crashed
        true
    }
}

fn exponential_backoff_time_with_upper_limit(time_gap: i64, power: u32, upper_limit: i64) -> bool {
    if time_gap > upper_limit {
        true
    } else {
        time_gap > 2_i64.pow(power)
    }
}

#[cfg(test)]
mod tests {
    use crate::statemachine::exponential_backoff_time_with_upper_limit;

    #[test]
    fn exponential_backoff_hit_limit() {
        assert!(exponential_backoff_time_with_upper_limit(15, 4, 8));
    }
    #[test]
    #[allow(clippy::bool_assert_comparison)]
    fn exponential_backoff_not_hit_limit() {
        assert_eq!(
            false,
            exponential_backoff_time_with_upper_limit(30, 5, 1024)
        );

        assert!(exponential_backoff_time_with_upper_limit(244, 5, 1024))
    }
}
