use opentelemetry::{metrics::Meter, Key};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::instrument;

const HOST_VERSION_KEY: Key = Key::from_static_str("bottlerocket_version");
const HOST_STATE_KEY: Key = Key::from_static_str("state");

#[derive(Debug)]
pub struct BrupopControllerMetrics {
    brupop_shared_hosts_data: Arc<Mutex<BrupopHostsData>>,
}

#[derive(Debug)]
pub struct BrupopHostsData {
    hosts_version_count_map: HashMap<String, u64>,
    hosts_state_count_map: HashMap<String, u64>,
}

impl BrupopHostsData {
    pub fn new(
        hosts_version_count_map: HashMap<String, u64>,
        hosts_state_count_map: HashMap<String, u64>,
    ) -> Self {
        BrupopHostsData {
            hosts_version_count_map,
            hosts_state_count_map,
        }
    }
}

impl Default for BrupopHostsData {
    fn default() -> Self {
        let hosts_version_count_map = HashMap::new();
        let hosts_state_count_map = HashMap::new();
        BrupopHostsData {
            hosts_version_count_map,
            hosts_state_count_map,
        }
    }
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

        let _ = meter.register_callback(move |cx| {
            let data = hosts_data_clone_for_version.lock().unwrap();
            for (host_version, count) in &data.hosts_version_count_map {
                let labels = vec![HOST_VERSION_KEY.string(host_version.clone())];
                brupop_hosts_version_observer.observe(cx, *count, &labels);
            }
        });

        let _ = meter.register_callback(move |cx| {
            let data = hosts_data_clone_for_state.lock().unwrap();
            for (host_state, count) in &data.hosts_state_count_map {
                let labels = vec![HOST_STATE_KEY.string(host_state.clone())];
                brupop_hosts_state_observer.observe(cx, *count, &labels);
            }
        });

        BrupopControllerMetrics {
            brupop_shared_hosts_data,
        }
    }

    /// Update shared mut ref to trigger ValueRecorder observe data.
    pub fn emit_metrics(&self, data: BrupopHostsData) {
        if let Ok(mut host_data) = self.brupop_shared_hosts_data.try_lock() {
            *host_data = data;
        }
    }
}
