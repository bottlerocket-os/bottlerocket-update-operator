use opentelemetry::{
    metrics::{Counter, Meter},
    Key,
};

const OPERATION_KEY: Key = Key::from_static_str("operation");

pub struct BrupopControllerMetrics {
    brupop_controller_op: Counter<u64>,
}

impl BrupopControllerMetrics {
    pub fn new(meter: Meter) -> Self {
        let brupop_controller_op = meter
            .u64_counter("brupop_controller_op")
            .with_description("Brupop controller operations")
            .init();
        BrupopControllerMetrics {
            brupop_controller_op,
        }
    }

    pub fn no_op(&self) {
        self.op("no_op".to_string());
    }

    pub fn op(&self, operation: String) {
        let labels = vec![OPERATION_KEY.string(operation)];
        self.brupop_controller_op.add(1, &labels);
    }
}
