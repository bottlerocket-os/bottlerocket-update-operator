//! Project-wide utility for initializing OpenTelemetry.
use opentelemetry::sdk::propagation::TraceContextPropagator;
use snafu::ResultExt;
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter, Registry};

const DEFAULT_TRACE_LEVEL: &str = "info";

pub fn init_telemetry_from_env() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_TRACE_LEVEL));
    let stdio_formatting_layer = fmt::layer().pretty();
    let subscriber = Registry::default()
        .with(env_filter)
        .with(stdio_formatting_layer);
    tracing::subscriber::set_global_default(subscriber)
        .context(error::TracingConfigurationSnafu)?;

    Ok(())
}

pub mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum TelemetryConfigError {
        #[snafu(display("Error configuring tracing: '{}'", source))]
        TracingConfiguration {
            source: tracing::subscriber::SetGlobalDefaultError,
        },
    }
}

type Result<T> = std::result::Result<T, TelemetryConfigError>;
pub use error::TelemetryConfigError;
