use models::node::{error, BottlerocketShadowClientError};

use actix_web::error::ResponseError;
use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Unable to parse HTTP header. Missing '{}'", missing_header))]
    HTTPHeaderParse { missing_header: &'static str },

    #[snafu(display("Unable to detect cluster IP family. For '{}'", source))]
    MissingClusterIPFamily { source: std::env::VarError },

    #[snafu(display("Error creating BottlerocketShadow: '{}'", source))]
    BottlerocketShadowCreate {
        source: BottlerocketShadowClientError,
    },

    #[snafu(display("Error patching BottlerocketShadow: '{}'", source))]
    BottlerocketShadowUpdate {
        source: BottlerocketShadowClientError,
    },

    #[snafu(display("Error running HTTP server: '{}'", source))]
    HttpServerError { source: std::io::Error },

    #[snafu(display("The Kubernetes WATCH on Pod objects has failed."))]
    KubernetesWatcherFailed {},

    #[snafu(display("Failed to cordon Node: '{}'", source))]
    BottlerocketShadowCordon {
        source: BottlerocketShadowClientError,
    },

    #[snafu(display("Failed to drain Node: '{}'", source))]
    BottlerocketShadowDrain {
        source: BottlerocketShadowClientError,
    },

    #[snafu(display("Failed to read certificate."))]
    ReadCertificateFailed { source: error::Error },

    #[snafu(display("Failed to reload certificate."))]
    ReloadCertificateFailed {},

    #[snafu(display("Failed to set up SslAcceptorBuilder : {:?}", source))]
    SSLError { source: openssl::error::ErrorStack },

    #[snafu(display("Failed to serialize Webhook response: {:?}", source))]
    WebhookError { source: serde_json::error::Error },
}

impl ResponseError for Error {}
