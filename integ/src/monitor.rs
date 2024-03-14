/*!
  monitor helps help automatically verify if that nodes are being
  updated to target Bottlerocket version.
!*/

use async_trait::async_trait;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams, ObjectList};
use snafu::OptionExt;
use snafu::ResultExt;
use std::time::SystemTime;

use tokio::time::{sleep, Duration};

use models::node::{BottlerocketShadow, BottlerocketShadowState};

const MONITOR_SLEEP_DURATION: Duration = Duration::from_secs(30);
const ESTIMATED_UPDATE_TIME_EACH_NODE: i32 = 300;
const EXTRA_TIME: i32 = 300;
const NUM_RETRIES: usize = 5;

pub type Result<T> = std::result::Result<T, monitor_error::Error>;

#[async_trait]
/// A trait providing an interface to interact with brupop recourse objects. This is provided as a trait
/// in order to allow mocks to be used for testing purposes.
pub trait BrupopClient: Clone + Sync + Send {
    // fetch BottlerocketShadows and help to get node `metadata`, `spec`, and  `status`.
    async fn fetch_shadows(&self) -> Result<ObjectList<BottlerocketShadow>>;
    // fetch brupop pods - Controllers, Agents, Apiserver to help on determining if they are on ideal status.
    async fn fetch_brupop_pods(&self) -> Result<ObjectList<Pod>>;
}

#[derive(Clone)]
/// Concrete implementation of the `BrupopClient` trait. This implementation will almost
/// certainly be used in any case that isn't a unit test.
pub struct IntegBrupopClient {
    k8s_client: kube::client::Client,
    namespace: String,
}

impl IntegBrupopClient {
    pub fn new(k8s_client: kube::client::Client, namespace: &str) -> Self {
        IntegBrupopClient {
            k8s_client,
            namespace: namespace.to_string(),
        }
    }
}

#[async_trait]
impl BrupopClient for IntegBrupopClient {
    async fn fetch_shadows(&self) -> Result<ObjectList<BottlerocketShadow>> {
        let brss: Api<BottlerocketShadow> =
            Api::namespaced(self.k8s_client.clone(), &self.namespace);

        let brss_object_list = brss
            .list(&ListParams::default())
            .await
            .context(monitor_error::FindBrupopPodsSnafu {})?;

        Ok(brss_object_list)
    }

    async fn fetch_brupop_pods(&self) -> Result<ObjectList<Pod>> {
        let pods: Api<Pod> = Api::namespaced(self.k8s_client.clone(), &self.namespace);
        let pods_objectlist = pods
            .list(&ListParams::default())
            .await
            .context(monitor_error::FindBrupopPodsSnafu {})?;

        Ok(pods_objectlist)
    }
}

#[async_trait]
/// A trait providing an interface to monitor brupop process.
pub trait Monitor: Clone {
    async fn run_monitor(&self) -> Result<()>;
}

#[derive(Clone)]
pub struct BrupopMonitor<T: BrupopClient> {
    integ_brupop_client: T,
}

impl<T: BrupopClient> BrupopMonitor<T> {
    pub fn new(integ_brupop_client: T) -> Self {
        BrupopMonitor {
            integ_brupop_client,
        }
    }

    // verify if Brupop pods (agent, api-server, controller) are in `running` status.
    fn check_pods_health(&self, pods: &ObjectList<Pod>) -> bool {
        if pods.items.is_empty() {
            false
        } else {
            return pods.iter().all(is_pod_running);
        }
    }

    // verify if brs has been created properly and initialized `status`.
    fn check_shadows_health(&self, bottlerocketshadows: &ObjectList<BottlerocketShadow>) -> bool {
        if bottlerocketshadows.items.is_empty() {
            false
        } else {
            return bottlerocketshadows
                .iter()
                .all(|bottlerocketshadow| bottlerocketshadow.status.is_some());
        }
    }

    // confirm that the instances successfully made it to the target version and the Idle state
    async fn confirm_update_success(
        &self,
        bottlerocketshadows: &ObjectList<BottlerocketShadow>,
    ) -> Result<bool> {
        let mut update_success = true;

        for bottlerocketshadow in bottlerocketshadows {
            let bottlerocket_shadow_status = bottlerocketshadow
                .status
                .as_ref()
                .context(monitor_error::MissingBottlerocketShadowStatusSnafu)?;
            if bottlerocket_shadow_status.current_version().to_string()
                != bottlerocket_shadow_status.target_version().to_string()
                || bottlerocket_shadow_status.current_state != BottlerocketShadowState::Idle
            {
                update_success &= false;
            }
            println!(
                "brs: {:?}      current_version: {:?}       current_state: {:?}",
                bottlerocketshadow
                    .metadata
                    .name
                    .as_ref()
                    .context(monitor_error::BottlerocketShadowNameSnafu)?,
                bottlerocket_shadow_status.current_version().to_string(),
                bottlerocket_shadow_status.current_state
            );
        }
        Ok(update_success)
    }
}

#[async_trait]
impl<T: BrupopClient> Monitor for BrupopMonitor<T> {
    async fn run_monitor(&self) -> Result<()> {
        let start_time = SystemTime::now();
        let mut retry_count = 0;

        loop {
            // fetch brupop pods (agent, api-server, controller) and brs to get latest info.
            let bottlerocketshadows = self.integ_brupop_client.fetch_shadows().await?;
            let pods = self.integ_brupop_client.fetch_brupop_pods().await?;

            // verify if Brupop pods (agent, api-server, controller) in `running` status
            // and if BottlerocketShadows (brs) are created properly.
            if !self.check_pods_health(&pods) || !self.check_shadows_health(&bottlerocketshadows) {
                if retry_count < NUM_RETRIES {
                    retry_count += 1;
                    sleep(MONITOR_SLEEP_DURATION).await;
                    continue;
                } else {
                    return Err(monitor_error::Error::BrupopMonitor {object: "Brupop pods (agent, apisever, controller or BottlerocketShadows) aren't on healthy status".to_string()});
                }
            }

            // verify if all instances are being updated
            if self.confirm_update_success(&bottlerocketshadows).await? {
                println!("[Complete]: All nodes have been successfully updated to latest version!");
                return Ok(());
            }

            // terminate monitor loop if time exceeds estimated update time
            if start_time
                .elapsed()
                .context(monitor_error::TimeElapsedSnafu {})?
                >= Duration::from_secs(estimate_expire_time(
                    bottlerocketshadows.into_iter().len() as i32
                ) as u64)
            {
                return Err(monitor_error::Error::BrupopMonitor {
                    object: "Monitor exceeds the estimated update time limit".to_string(),
                });
            }

            println!("[Not ready] keep monitoring!");
            sleep(MONITOR_SLEEP_DURATION).await;
        }
    }
}

#[cfg(any(feature = "mockall", test))]
pub mod mock {
    use super::*;
    use mockall::{mock, predicate::*};
    mock! {
        /// A Mock BrupopClient for use in tests.
        pub BrupopClient {}
        #[async_trait]
        impl BrupopClient for BrupopClient {
            async fn fetch_shadows(&self) -> Result<ObjectList<BottlerocketShadow>>;
            async fn fetch_brupop_pods(&self) -> Result<ObjectList<Pod>>;
        }

        impl Clone for BrupopClient {
            fn clone(&self) -> Self;
        }
    }

    mock! {
        /// A Mock  for use in tests.
        pub Monitor {}
        #[async_trait]
        impl Monitor for Monitor {
            async fn run_monitor(&self) -> Result<()>;

        }

        impl Clone for Monitor {
            fn clone(&self) -> Self;
        }
    }
}

// compute the estimated update time to trigger monitor exit
// formula: number_of_node*300 secs + 300 secs
fn estimate_expire_time(number_of_brs: i32) -> i32 {
    number_of_brs * ESTIMATED_UPDATE_TIME_EACH_NODE + EXTRA_TIME
}

fn is_pod_running(pod: &Pod) -> bool {
    pod.status
        .as_ref()
        .and_then(|s| s.phase.as_ref().map(|s| s == "Running"))
        .unwrap_or(false)
}

pub mod monitor_error {
    use std::time::SystemTimeError;

    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Unable to find Brupop pods: {}", source))]
        FindBrupopPods { source: kube::Error },

        #[snafu(display(
            "Failed to run brupop monitor because {}, please check brupop pods' logs",
            object
        ))]
        BrupopMonitor { object: String },

        #[snafu(display("Unable to get Bottlerocket name"))]
        BottlerocketShadowName,

        #[snafu(display(
            "Unable to get Bottlerocket node 'status' because of missing 'status' value"
        ))]
        MissingBottlerocketShadowStatus,

        #[snafu(display(
            "Unable to fetch {} store: Store unavailable: retries exhausted",
            object
        ))]
        ReflectorUnavailable { object: String },

        #[snafu(display(
            "Unable to return the difference between the clock time when this system time was created, and the current clock time."
        ))]
        TimeElapsed { source: SystemTimeError },
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::mock::MockBrupopClient;
    use super::*;
    use chrono::Utc;
    use semver::Version;
    use std::str::FromStr;

    use k8s_openapi::api::core::v1::{Pod, PodStatus};
    use kube::api::{ListMeta, ObjectList, ObjectMeta, TypeMeta};

    use models::node::{BottlerocketShadow, BottlerocketShadowState, BottlerocketShadowStatus};

    pub(crate) fn fake_pod(pod_status: String) -> Pod {
        Pod {
            status: Some(PodStatus {
                phase: Some(pod_status),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_check_brupop_pods_health() {
        let brupop_monitor = BrupopMonitor::new(MockBrupopClient::new());
        let mut test_cases = vec![
            (
                ObjectList {
                    items: vec![
                        fake_pod("Running".to_string()),
                        fake_pod("Running".to_string()),
                        fake_pod("Running".to_string()),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                true,
            ),
            (
                ObjectList {
                    items: vec![
                        fake_pod("Failed".to_string()),
                        fake_pod("Running".to_string()),
                        fake_pod("Failed".to_string()),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
            (
                ObjectList {
                    items: vec![],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
        ];
        for (pods, is_healthy) in test_cases.drain(..) {
            let result = brupop_monitor.check_pods_health(&pods);
            assert_eq!(result, is_healthy);
        }
    }

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
                Some(Utc::now()),
            )),
            metadata: ObjectMeta {
                name: Some(name),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_check_shadows_health() {
        let brupop_monitor = BrupopMonitor::new(MockBrupopClient::new());
        let mut test_cases = vec![
            (
                ObjectList {
                    items: vec![
                        fake_shadow(
                            "brs-ip-1.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-2.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-3.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                true,
            ),
            (
                ObjectList {
                    items: vec![
                        BottlerocketShadow {
                            ..Default::default()
                        },
                        BottlerocketShadow {
                            ..Default::default()
                        },
                        BottlerocketShadow {
                            ..Default::default()
                        },
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
            (
                ObjectList {
                    items: vec![],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
        ];

        for (brss, is_healthy) in test_cases.drain(..) {
            let result = brupop_monitor.check_shadows_health(&brss);
            assert_eq!(result, is_healthy);
        }
    }

    #[tokio::test]
    async fn test_confirm_update_success() {
        let brupop_monitor = BrupopMonitor::new(MockBrupopClient::new());
        let mut test_cases = vec![
            (
                ObjectList {
                    items: vec![
                        fake_shadow(
                            "brs-ip-1.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-2.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::StagedAndPerformedUpdate,
                        ),
                        fake_shadow(
                            "brs-ip-3.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
            (
                ObjectList {
                    items: vec![
                        fake_shadow(
                            "brs-ip-1.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.6.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-2.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.6.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-3.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.6.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                true,
            ),
            (
                ObjectList {
                    items: vec![
                        fake_shadow(
                            "brs-ip-1.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.6.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                        fake_shadow(
                            "brs-ip-2.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::StagedAndPerformedUpdate,
                        ),
                        fake_shadow(
                            "brs-ip-3.us-west-2.compute.internal".to_string(),
                            "1.6.0".to_string(),
                            "1.5.0".to_string(),
                            BottlerocketShadowState::Idle,
                        ),
                    ],
                    metadata: ListMeta {
                        continue_: None,
                        remaining_item_count: None,
                        resource_version: Some("83212702".to_string()),
                        self_link: None,
                    },
                    types: TypeMeta {
                        api_version: "v1".to_string(),
                        kind: "Custom".to_string(),
                    },
                },
                false,
            ),
        ];

        for (brss, is_update_complete) in test_cases.drain(..) {
            let result = brupop_monitor.confirm_update_success(&brss).await.unwrap();
            assert_eq!(result, is_update_complete);
        }
    }
}
