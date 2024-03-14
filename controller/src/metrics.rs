use models::node::BottlerocketShadow;
use opentelemetry::{metrics::Meter, Key};
use snafu::ResultExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::instrument;

const HOST_VERSION_KEY: Key = Key::from_static_str("bottlerocket_version");
const HOST_STATE_KEY: Key = Key::from_static_str("state");

#[derive(Debug)]
pub struct BrupopControllerMetrics {
    brupop_shared_hosts_data: Arc<Mutex<BrupopHostsData>>,
}

#[derive(Debug, Default)]
pub struct BrupopHostsData {
    hosts_version_count: HashMap<String, u64>,
    hosts_state_count: HashMap<String, u64>,
}

impl BrupopHostsData {
    /// Computes point-in-time metrics for the cluster's hosts based on a set of BottlerocketShadows.
    pub fn from_shadows(brss: &[BottlerocketShadow]) -> Result<Self, error::MetricsError> {
        let mut hosts_version_count = HashMap::new();
        let mut hosts_state_count = HashMap::new();

        brss.iter()
            .filter_map(|brs| brs.status.as_ref())
            .try_for_each(|brs_status| {
                let host_version = brs_status.current_version().to_string();
                let host_state = brs_status.current_state;

                *hosts_version_count.entry(host_version).or_default() += 1;
                *hosts_state_count
                    .entry(serde_plain::to_string(&host_state).context(error::SerializeStateSnafu)?)
                    .or_default() += 1;

                Ok(())
            })?;
        Ok(Self {
            hosts_version_count,
            hosts_state_count,
        })
    }

    /// Marks all current gauges at 0, then writes the new metrics into the store.
    fn update_counters(&mut self, other: &BrupopHostsData) {
        update_counter(&mut self.hosts_version_count, &other.hosts_version_count);
        update_counter(&mut self.hosts_state_count, &other.hosts_state_count);
    }
}

/// Updates a population counter from a stateless input.
///
/// All current state in the counter is set to 0, then new counts are copied from the incoming state.
fn update_counter(base: &mut HashMap<String, u64>, other: &HashMap<String, u64>) {
    base.iter_mut().for_each(|(_k, v)| *v = 0);

    other.iter().for_each(|(k, v)| {
        *base.entry(k.clone()).or_default() = *v;
    });
}

impl BrupopControllerMetrics {
    #[instrument]
    pub fn new(meter: Meter) -> Self {
        let brupop_shared_hosts_data = Arc::new(Mutex::new(BrupopHostsData::default()));
        let hosts_data_clone_for_version = Arc::clone(&brupop_shared_hosts_data);
        let hosts_data_clone_for_state = Arc::clone(&brupop_shared_hosts_data);

        // Observer for cluster host's bottlerocket version
        let brupop_hosts_version_observer = meter
            .u64_observable_gauge("brupop_hosts_version")
            .with_description("Brupop host's bottlerocket version")
            .init();

        // Observer for cluster host's brupop state
        let brupop_hosts_state_observer = meter
            .u64_observable_gauge("brupop_hosts_state")
            .with_description("Brupop host's state")
            .init();

        let _ = meter.register_callback(&[brupop_hosts_version_observer.as_any()], move |cx| {
            let data = hosts_data_clone_for_version.lock().unwrap();
            for (host_version, count) in &data.hosts_version_count {
                let labels = vec![HOST_VERSION_KEY.string(host_version.to_string())];
                cx.observe_u64(&brupop_hosts_version_observer, *count, &labels);
            }
        });

        let _ = meter.register_callback(&[brupop_hosts_state_observer.as_any()], move |cx| {
            let data = hosts_data_clone_for_state.lock().unwrap();
            for (host_state, count) in &data.hosts_state_count {
                let labels = vec![HOST_STATE_KEY.string(host_state.to_string())];
                cx.observe_u64(&brupop_hosts_state_observer, *count, &labels);
            }
        });

        BrupopControllerMetrics {
            brupop_shared_hosts_data,
        }
    }

    /// Update shared mut ref to trigger ValueRecorder observe data.
    pub fn emit_metrics(&self, data: BrupopHostsData) {
        if let Ok(mut host_data) = self.brupop_shared_hosts_data.try_lock() {
            host_data.update_counters(&data);
        }
    }
}

pub mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum MetricsError {
        #[snafu(display("Failed to serialize Shadow state: '{}'", source))]
        SerializeState { source: serde_plain::Error },
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use maplit::hashmap;

    use crate::metrics::update_counter;

    #[test]
    fn test_update_counter() {
        let test_cases = vec![
            (
                hashmap! {
                    "a" => 5,
                    "b" => 10,
                    "c" => 15,
                },
                hashmap! {
                    "a" => 11,

                },
                hashmap! {
                    "a" => 11,
                    "b" => 0,
                    "c" => 0,
                },
            ),
            (
                hashmap! {
                    "a" => 1,
                },
                hashmap! {
                    "b" => 11,
                    "c" => 12,
                },
                hashmap! {
                    "a" => 0,
                    "b" => 11,
                    "c" => 12,
                },
            ),
            (
                hashmap! {
                    "a" => 1,
                },
                hashmap! {
                    "a" => 2,
                },
                hashmap! {
                    "a" => 2,
                },
            ),
        ];

        fn stringify(hashmap: HashMap<&str, u64>) -> HashMap<String, u64> {
            hashmap
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()
        }

        for (base, other, expected) in test_cases.into_iter() {
            let mut base = stringify(base);
            let other = stringify(other);
            let expected = stringify(expected);

            update_counter(&mut base, &other);
            assert_eq!(&base, &expected);
        }
    }
}
