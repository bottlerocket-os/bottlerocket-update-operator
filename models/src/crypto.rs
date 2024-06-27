//! Installs a default `CryptoProvider` for use with `rustls`.
//!
//! While the default *should* be correctly selected based on the feature flags chosen for rustls,
//! it is easy to accidentally enable multiple providers and make the default selection ambiguous
//! for `rustls`, which results in a panic.
//!
//! This function will panic if a crypto provider cannot be installed.

use rustls::crypto::CryptoProvider;
use snafu::Snafu;

pub fn install_default_crypto_provider() -> Result<(), CryptoConfigError> {
    CryptoProvider::install_default(rustls::crypto::ring::default_provider())
        .map_err(|_| CryptoConfigError)
}

#[derive(Debug, Snafu)]
#[snafu(display("Failed to install crypto provider."), visibility(pub))]
pub struct CryptoConfigError;
