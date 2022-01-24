//! Provides an implementation for draining Pods from a Kubernetes Node, similar to `kubectl drain`.
//!
//! Draining in Kubernetes is done client side, and is typically a combination of "cordoning" a Node by
//! marking it as unschedulable, followed by deleting (or evicting, which is a distinct concept) Pods from the
//! Node. This implementation uses evictions, which respect PodDisruptionBudgets (PDBs).
//!
//! Cordoning is not handled here, because `kube-rs` provides `Api::cordon()`.
use futures::{stream, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{EvictParams, ListParams},
    Api, ResourceExt,
};
use reqwest::StatusCode;
use snafu::ResultExt;
use tokio::time::{sleep, Duration, Instant};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    RetryIf,
};
use tracing::{event, instrument, Level};

// Maximum number of Pods to evict concurrently. Waiting for Pods to be deleted is included in this limitation.
// Eviction retries are slow under typical conditions, but we don't want to generate too many TPS to Kubernetes.
// Keeping this relatively low is probably a good idea.
const CONCURRENT_EVICTIONS: usize = 5;

// When waiting for a PodDisruptionBudget to be satisfied, or if there is a server error, we stall for a fixed rate between eviction attempts.
// `kubectl drain` similarly waits 5 seconds between eviction attempts.
const EVICTION_RETRY_INTERVAL: Duration = Duration::from_secs(5);

// After evictions are created, we wait for the Pods to be deleted by Kubernetes.
// These constants define the poll interval for checking the Pods, as well as the max amount of time to wait.
const DELETION_CHECK_INTERVAL: Duration = Duration::from_secs(5);
// `kubectl drain` by default will wait "forever" for an eviction to complete. We follow suit.
const DELETION_TIMEOUT: Duration = Duration::from_secs(u64::MAX);

// Some errors while attempting evictions result in retries with exponential backoff.
// These values configure how long to delay between tries.
// We should be tenacious in attempting retries, as some workloads are sensitive to being suddenly interrupted.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(20);
const NUM_RETRIES: usize = 10;

/// Wrapper struct to provide retry configurations for evictions.
struct RetryStrategy {}
impl RetryStrategy {
    fn retry_strategy() -> impl Iterator<Item = Duration> {
        ExponentialBackoff::from_millis(RETRY_BASE_DELAY.as_millis() as u64)
            .max_delay(RETRY_MAX_DELAY)
            .map(jitter)
            .take(NUM_RETRIES)
    }
}

impl tokio_retry::Condition<error::EvictionError> for RetryStrategy {
    fn should_retry(&mut self, error: &error::EvictionError) -> bool {
        error.should_retry()
    }
}

/// Drains a node of all pods.
///
/// The Kubernetes API does not provide an implementation of drain. You must use Pod deletion or the Eviction API
/// to remove all Pods from a given Node. We opt to use Evictions in order to respect Pod Disruption Budgets.
///
/// The implementation of `kubectl drain` can be used as inspiration. While this implementation will halt under
/// certain special conditions, we have slightly different behavior:
///
/// Kubectl by default will not evict nodes under some criteria without further instruction.
/// By default, we ignore:
/// * DaemonSet Pods - The DaemonSet controller will not respect node cordons, so we don't battle it.
/// * Mirror Pods - These are static and cannot be controlled.
///
/// Otherwise, we evict pods that kubectl gives special care:
/// - Pods with local storage that will be deleted when drained (emptyDir).
/// - Unreplicated pods (Pods without a controller.)
///
/// PodDisruptionBudgets can be used to protect these workloads from being unduly interrupted.
#[instrument(skip(k8s_client), err)]
pub(crate) async fn drain_node(
    k8s_client: &kube::Client,
    node_name: &str,
) -> Result<(), error::DrainError> {
    let target_pods = find_target_pods(k8s_client, node_name).await?;

    // Perform the eviction for each pod simultaneously.
    stream::iter(target_pods)
        .for_each_concurrent(CONCURRENT_EVICTIONS, move |pod| {
            let k8s_client = k8s_client.clone();
            let pod = pod.clone();
            async move {
                // If an eviction for a Pod fails, it's either because:
                // * The eviction would never succeed (the Pod doesn't exist, we lack permissions to evict them, etc)
                // * The eviction may succeed, but we have retried many times and hit possibly transient errors.
                // In either case, a log message is emitted but we proceed with the drain, ultimately reporting success.
                // We want to avoid triggering an endless retry loop if we have mistakenly labelled an error as transient
                // when it is not.
                if evict_pod(&k8s_client, &pod).await.is_ok() {
                    // Deletions that do not complete within the given time limit are logged but ultimately ignored.
                    wait_for_deletion(&k8s_client, &pod).await.ok();
                }
            }
        })
        .await;

    Ok(())
}

/// Finds all pods on a given node that are targeted for eviction during a drain.
/// See documentation on [`drain_node`] for more information about which pods are selected.
#[instrument(skip(k8s_client), err)]
async fn find_target_pods(
    k8s_client: &kube::Client,
    node_name: &str,
) -> Result<impl Iterator<Item = Pod>, error::DrainError> {
    let pods: Api<Pod> = Api::all(k8s_client.clone());

    let our_pods = pods
        .list(&ListParams {
            field_selector: Some(format!("spec.nodeName={}", node_name)),
            ..Default::default()
        })
        .await
        .context(error::FindTargetPods {
            node_name: node_name.to_string(),
        })?;

    Ok(filter_pods(our_pods.into_iter()))
}

/// Given a list of all pods for a given node, this filters out pods which we do not want to attempt to drain.
/// By default, we skip daemonset and static Mirror pods.
fn filter_pods<F: Iterator<Item = Pod>>(pods: F) -> impl Iterator<Item = Pod> {
    pods.filter(|pod| {
        // Any completed pod can remain.
        if let Some(status) = pod.status.as_ref() {
            if let Some(phase) = status.phase.as_ref() {
                if phase == "Failed" || phase == "Succeeded" {
                    return true;
                }
            }
        }

        // Ignore daemonset pods, as the DaemonSet controller ignores node cordons.
        if let Some(owner_references) = pod.metadata.owner_references.as_ref() {
            if owner_references.iter().any(|reference| {
                reference.controller == Some(true) && reference.kind == "DaemonSet"
            }) {
                // TODO: Kubectl's drain evicts "orphaned" pods, where the owning DaemonSet no longer exists.
                event!(
                    Level::INFO,
                    "Not draining Pod '{}': Pod is member of a DaemonSet",
                    pod.name()
                );
                return false;
            }
        }

        // Ignore static mirror pods, they cannot be controlled.
        if let Some(annotations) = pod.metadata.annotations.as_ref() {
            if annotations.contains_key("kubernetes.io/config.mirror") {
                event!(
                    Level::INFO,
                    "Not draining Pod '{}': Pod is a static Mirror Pod",
                    pod.name()
                );
                return false;
            }
        }

        return true;
    })
}

#[instrument(skip(k8s_client, pod), err)]
/// Create an eviction for the desired Pod.
async fn evict_pod(k8s_client: &kube::Client, pod: &Pod) -> Result<(), error::EvictionError> {
    let pod_api = namespaced_pod_api(k8s_client, pod);

    // When evicting a node, a 429 (TOO_MANY_REQUESTS) response code is used to indicate that we must wait to allow a PodDisruptionBudget (PDB) to be satisfied.
    // If there is some kind of misconfiguration (e.g. multiple PDBs that refer to the same Pod), you get a 500.
    // For a given eviction request, there are two cases:
    // * No budget matches the pod. In this case, you always receive a 200 OK.
    // * There is at least one budget, in which case any of the above 3 responses (200, 429, 500) may apply.
    //
    // It's possible for an eviction to become stuck: the eviction API will never return anything other than 429 or 500. This would be due to invalid PDBs, or PDBs
    // which cannot be satisifed with the current cluster resources. In these cases, Brupop will continuously retry to evict rather than clobber an attempt to
    // protect cluster resources with PDBs. Operators must intervene manually.
    // See https://kubernetes.io/docs/tasks/administer-cluster/safely-drain-node/#stuck-evictions for details.
    RetryIf::spawn(RetryStrategy::retry_strategy(), || async {
        loop {
            event!(Level::INFO, "Attempting to evict pod {}", &pod.name());
            let eviction_result = pod_api.evict(&pod.name(), &EvictParams::default()).await;

            match eviction_result {
                Ok(_) => {
                    event!(Level::INFO, "Successfully evicted Pod '{}'", pod.name());
                    break;
                }
                Err(kube::Error::Api(e)) => {
                    let status_code = StatusCode::from_u16(e.code as u16);
                    match status_code {
                        Ok(StatusCode::TOO_MANY_REQUESTS) => {
                            event!(
                            Level::ERROR,
                            "Too many requests when creating Eviction for Pod '{}': '{}'. This is likely due to respecting a Pod Disruption Budget. Retrying in {:.2}s.",
                            pod.name(),
                            e,
                            EVICTION_RETRY_INTERVAL.as_secs_f64()
                        );
                            sleep(EVICTION_RETRY_INTERVAL).await;
                            continue;
                        }
                        Ok(StatusCode::INTERNAL_SERVER_ERROR) => {
                            event!(
                            Level::ERROR,
                            "Error when evicting Pod '{}': '{}'. Check for misconfigured PodDisruptionBudgets. Retrying in {:.2}s.",
                            pod.name(),
                            e,
                            EVICTION_RETRY_INTERVAL.as_secs_f64()
                        );
                            sleep(EVICTION_RETRY_INTERVAL).await;
                            continue;
                        }
                        Ok(StatusCode::NOT_FOUND) => {
                            return Err(error::EvictionError::NonRetriableEviction {
                                source: kube::Error::Api(e.clone()),
                                pod_name: pod.name().to_string(),
                            });
                        }
                        Ok(StatusCode::FORBIDDEN) => {
                            // An eviction request in a deleting namespace will throw a forbidden error.
                            // `kubectl drain` will still wait for these pods to be deleted; however, kube-rs does not provide granular enough access to
                            // API error statuses to determine if we can proceed, so we ignore these.
                            return Err(error::EvictionError::NonRetriableEviction {
                                source: kube::Error::Api(e.clone()),
                                pod_name: pod.name().to_string(),
                            });
                        }
                        Ok(_) => {
                            event!(
                                Level::ERROR,
                                "Error when evicting Pod '{}': '{}'.",
                                pod.name(),
                                e
                            );
                            return Err(error::EvictionError::RetriableEviction {
                                source: kube::Error::Api(e.clone()),
                                pod_name: pod.name().to_string(),
                            });
                        }
                        Err(_) => {
                            event!(
                                Level::ERROR,
                                "Received invalid response code from Kubernetes API: '{}'",
                                e
                            );
                            return Err(error::EvictionError::RetriableEviction {
                                source: kube::Error::Api(e.clone()),
                                pod_name: pod.name().to_string(),
                            });
                        }
                    }
                }
                Err(e) => {
                    event!(Level::ERROR, "Eviction failed: '{}'. Retrying...", e);
                    return Err(error::EvictionError::RetriableEviction {
                        source: e,
                        pod_name: pod.name().to_string(),
                    });
                }
            }
        }
        Ok(())
    }, RetryStrategy {}).await
}

#[instrument(skip(k8s_client, pod), err)]
/// Wait for the given Pod to be deleted by Kubernetes.
async fn wait_for_deletion(k8s_client: &kube::Client, pod: &Pod) -> Result<(), error::DrainError> {
    let start_time = Instant::now();

    let pod_api = namespaced_pod_api(k8s_client, pod);
    loop {
        match pod_api.get(&pod.name()).await {
            Err(kube::Error::Api(e)) if e.code == 404 => {
                event!(Level::INFO, "Pod {} deleted.", pod.name(),);
                break;
            }
            Ok(_) => {
                event!(
                    Level::DEBUG,
                    "Pod '{}' not yet deleted. Waiting {}s.",
                    pod.name(),
                    DELETION_CHECK_INTERVAL.as_secs_f64()
                );
            }

            Err(e) => {
                event!(
                    Level::ERROR,
                    "Could not determine if Pod '{}' has been deleted: '{}'. Waiting {}s.",
                    pod.name(),
                    e,
                    DELETION_CHECK_INTERVAL.as_secs_f64()
                );
            }
        }
        if start_time.elapsed() > DELETION_TIMEOUT {
            return Err(error::DrainError::WaitForDeletion {
                pod_name: pod.name(),
                max_wait: DELETION_TIMEOUT,
            });
        } else {
            sleep(DELETION_CHECK_INTERVAL).await;
        }
    }
    Ok(())
}

/// Creates a kube::Api<Pod> for interacting with Pods in the namespace associated with the given Pod.
fn namespaced_pod_api(k8s_client: &kube::Client, pod: &Pod) -> Api<Pod> {
    match pod.metadata.namespace.as_ref() {
        Some(ns) => Api::namespaced(k8s_client.clone(), &ns),
        None => Api::default_namespaced(k8s_client.clone()),
    }
}

pub mod error {
    use snafu::Snafu;
    use tokio::time::Duration;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum DrainError {
        #[snafu(display("Unable to find drainable Pods for Node '{}': '{}'", node_name, source))]
        FindTargetPods {
            source: kube::Error,
            node_name: String,
        },

        #[snafu(display("Pod '{}' was not deleted in the time allocated ({:.2}s).", pod_name, max_wait.as_secs_f64()))]
        WaitForDeletion {
            pod_name: String,
            max_wait: Duration,
        },
    }

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
    pub enum EvictionError {
        #[snafu(display("Unable to create eviction for Pod '{}': '{}'", pod_name, source))]
        /// An error occurred while attempting to evict a Pod. This may result in an attempt to retry the eviction.
        RetriableEviction {
            source: kube::Error,
            pod_name: String,
        },

        #[snafu(display("Unable to create eviction for Pod '{}': '{}'", pod_name, source))]
        /// A fatal error occurred while attempting to evict a Pod. This will not be retried.
        NonRetriableEviction {
            source: kube::Error,
            pod_name: String,
        },
    }

    impl EvictionError {
        pub fn should_retry(&self) -> bool {
            match self {
                Self::RetriableEviction { .. } => true,
                Self::NonRetriableEviction { .. } => false,
            }
        }
    }
}
