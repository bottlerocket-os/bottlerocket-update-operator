use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub struct ProviderError {
    /// Any message to be included with the error. This will be included in the formatted display
    /// before `inner`.
    context: Option<String>,
    /// The error that caused this error.
    inner: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

/// The result type returned by instance create and termiante operations.
pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

impl ProviderError {
    pub fn new_with_source_and_context<S, E>(context: S, source: E) -> Self
    where
        S: Into<String>,
        E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    {
        Self {
            context: Some(context.into()),
            inner: Some(source.into()),
        }
    }

    pub fn new_with_source<E>(source: E) -> Self
    where
        E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    {
        Self {
            context: None,
            inner: Some(source.into()),
        }
    }

    pub fn new_with_context<S>(context: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            context: Some(context.into()),
            inner: None,
        }
    }

    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    pub fn inner(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.inner.as_ref().map(|some| some.as_ref())
    }
}

impl Display for ProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(context) = self.context() {
            write!(f, ", {}", context)?;
        }
        if let Some(inner) = self.inner() {
            write!(f, ": {}", inner)?;
        }
        Ok(())
    }
}

// Make `ProviderError` function as a standard error.
impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

/// A trait that makes it possible to convert error types to `ProviderError` using a familiar
/// `context` function.
pub trait IntoProviderError<T> {
    /// Convert `self` into a `ProviderError`.
    fn context<S>(self, message: S) -> ProviderResult<T>
    where
        S: Into<String>;
}

// Implement `IntoProviderError` for all standard `Error + Send + Sync + 'static` types.
impl<T, E> IntoProviderError<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context<S>(self, message: S) -> ProviderResult<T>
    where
        S: Into<String>,
    {
        self.map_err(|e| ProviderError::new_with_source_and_context(message, e))
    }
}

// Implement `IntoProviderError` for options where `None` is converted into an error.
impl<T> IntoProviderError<T> for std::option::Option<T> {
    fn context<S>(self, m: S) -> Result<T, ProviderError>
    where
        S: Into<String>,
    {
        self.ok_or_else(|| ProviderError::new_with_context(m))
    }
}
