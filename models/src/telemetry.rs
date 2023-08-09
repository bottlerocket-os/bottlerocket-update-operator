//! Project-wide utility for initializing OpenTelemetry.
use opentelemetry::sdk::propagation::TraceContextPropagator;
use serde::Deserialize;
use snafu::ResultExt;
use std::env;
use tracing::Subscriber;
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, EnvFilter, Registry};

const DEFAULT_TRACING_FILTER_DIRECTIVE: LevelFilter = LevelFilter::INFO;

const TRACING_FILTER_DIRECTIVE_ENV_VAR: &str = "TRACING_FILTER_DIRECTIVE";
const LOGGING_FORMATTER_ENV_VAR: &str = "LOGGING_FORMATTER";
const LOGGING_ANSI_ENABLED_ENV_VAR: &str = "LOGGING_ANSI_ENABLED";

/// The formatter for logging tracing events.
///
/// Controls the format of the message as well as whether or not to enable ANSI colors.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub struct LogFormatter {
    message_format: MessageFormat,
    ansi_enabled: bool,
}

impl LogFormatter {
    pub fn try_from_env() -> Result<Self> {
        let message_format = MessageFormat::try_from_env()?;
        let ansi_enabled = Self::ansi_enabled_from_env()?;

        Ok(Self {
            message_format,
            ansi_enabled,
        })
    }

    fn ansi_enabled_from_env() -> Result<bool> {
        env::var(LOGGING_ANSI_ENABLED_ENV_VAR)
            .ok()
            .map(|ansi_enabled_str| {
                ansi_enabled_str
                    .to_lowercase()
                    .parse()
                    .context(error::LogAnsiEnvSnafu {
                        env_value: ansi_enabled_str.to_string(),
                    })
            })
            .unwrap_or(Ok(false))
    }

    /// Adds a formatting layer to a tracing event subscriber.
    fn add_format_layer<S>(&self, event_subscriber: S) -> Box<dyn Subscriber + Send + Sync>
    where
        S: SubscriberExt + Send + Sync + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        // Quite repitious, but the layers are all different types and we can't Box them, the subscriber won't allow it.
        match self.message_format {
            MessageFormat::Full => {
                Box::new(event_subscriber.with(fmt::layer().with_ansi(self.ansi_enabled)))
            }
            MessageFormat::Compact => {
                Box::new(event_subscriber.with(fmt::layer().compact().with_ansi(self.ansi_enabled)))
            }
            MessageFormat::Pretty => {
                Box::new(event_subscriber.with(fmt::layer().pretty().with_ansi(self.ansi_enabled)))
            }
            MessageFormat::Json => {
                Box::new(event_subscriber.with(fmt::layer().json().with_ansi(self.ansi_enabled)))
            }
        }
    }
}

/// The message format for logging tracing events.
///
/// See https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/format/index.html
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MessageFormat {
    /// Human-readable, single-line logs for each event.
    Full,
    /// A variant of the default formatter optimized for short line lengths.
    Compact,
    #[default]
    /// Pretty-formatted multi-line logs optimized for human readability.
    Pretty,
    /// Newline-delimited JSON logs.
    Json,
}

impl MessageFormat {
    pub fn try_from_env() -> Result<Self> {
        env::var(LOGGING_FORMATTER_ENV_VAR)
            .ok()
            .map(|formatter| {
                serde_plain::from_str(&formatter).context(error::LogFormatterEnvSnafu {
                    env_value: formatter,
                })
            })
            .unwrap_or(Ok(Default::default()))
    }
}

pub fn init_telemetry_from_env() -> Result<()> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = EnvFilter::builder()
        .with_default_directive(DEFAULT_TRACING_FILTER_DIRECTIVE.into())
        .with_env_var(TRACING_FILTER_DIRECTIVE_ENV_VAR)
        .from_env_lossy();

    let subscriber = Registry::default().with(env_filter);
    let subscriber = LogFormatter::try_from_env()?.add_format_layer(subscriber);

    tracing::subscriber::set_global_default(subscriber)
        .context(error::TracingConfigurationSnafu)?;

    Ok(())
}

pub mod error {
    use std::str::ParseBoolError;

    use super::*;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum TelemetryConfigError {
        #[snafu(display("Error configuring tracing: '{}'", source))]
        TracingConfiguration {
            source: tracing::subscriber::SetGlobalDefaultError,
        },

        #[snafu(display(
            "Could not parse formatter from environment variable '{}={}': '{}'",
            LOGGING_FORMATTER_ENV_VAR,
            env_value,
            source
        ))]
        LogFormatterEnv {
            source: serde_plain::Error,
            env_value: String,
        },

        #[snafu(display(
            "Could not parse ANSI enablement from environment variable '{}={}': '{}'",
            LOGGING_ANSI_ENABLED_ENV_VAR,
            env_value,
            source
        ))]
        LogAnsiEnv {
            source: ParseBoolError,
            env_value: String,
        },
    }
}

type Result<T> = std::result::Result<T, TelemetryConfigError>;
pub use error::TelemetryConfigError;
