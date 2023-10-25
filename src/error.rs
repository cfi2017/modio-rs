//! Client errors
use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;

use crate::auth::Error as AuthError;
use crate::types::Error as ModioError;

/// A `Result` alias where the `Err` case is `modio::Error`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The Errors that may occur when using `Modio`.
pub struct Error {
    inner: Box<Inner>,
}

type BoxError = Box<dyn StdError + Send + Sync>;

struct Inner {
    kind: Kind,
    error_ref: Option<u16>,
    source: Option<BoxError>,
}

impl Error {
    #[inline]
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            inner: Box::new(Inner {
                kind,
                error_ref: None,
                source: None,
            }),
        }
    }

    #[inline]
    pub(crate) fn with<E: Into<BoxError>>(mut self, source: E) -> Self {
        self.inner.source = Some(source.into());
        self
    }

    #[inline]
    pub(crate) fn with_error_ref(mut self, error_ref: u16) -> Self {
        self.inner.error_ref = Some(error_ref);
        self
    }

    /// Returns true if the API key/access token is incorrect, revoked, expired or the request
    /// needs a different authentication method.
    pub fn is_auth(&self) -> bool {
        matches!(
            self.inner.kind,
            Kind::Auth(AuthError::Unauthorized | AuthError::TokenRequired)
        )
    }

    /// Returns true if the acceptance of the Terms of Use is required before continuing external
    /// authorization.
    pub fn is_terms_acceptance_required(&self) -> bool {
        use AuthError::TermsAcceptanceRequired;
        matches!(self.inner.kind, Kind::Auth(TermsAcceptanceRequired))
    }

    /// Returns true if the error is from a type Builder.
    pub fn is_builder(&self) -> bool {
        matches!(self.inner.kind, Kind::Builder)
    }

    /// Returns true if the error is from a [`DownloadAction`](crate::download::DownloadAction).
    pub fn is_download(&self) -> bool {
        matches!(self.inner.kind, Kind::Download)
    }

    /// Returns true if the rate limit associated with credentials has been exhausted.
    pub fn is_ratelimited(&self) -> bool {
        matches!(self.inner.kind, Kind::RateLimit { .. })
    }

    /// Returns true if the error was generated from a response.
    pub fn is_status(&self) -> bool {
        matches!(self.inner.kind, Kind::Status(_))
    }

    /// Returns true if the error contains validation errors.
    pub fn is_validation(&self) -> bool {
        matches!(self.inner.kind, Kind::Validation { .. })
    }

    /// Returns true if the error is related to serialization.
    pub fn is_decode(&self) -> bool {
        matches!(self.inner.kind, Kind::Decode)
    }

    /// Returns modio's error reference code.
    ///
    /// See the [Error Codes](https://docs.mod.io/#error-codes) docs for more information.
    pub fn error_ref(&self) -> Option<u16> {
        self.inner.error_ref
    }

    /// Returns status code if the error was generated from a response.
    pub fn status(&self) -> Option<StatusCode> {
        match self.inner.kind {
            Kind::Status(code) => Some(code),
            _ => None,
        }
    }

    /// Returns validation message & errors from the response.
    pub fn validation(&self) -> Option<(&String, &Vec<(String, String)>)> {
        match self.inner.kind {
            Kind::Validation {
                ref message,
                ref errors,
            } => Some((message, errors)),
            _ => None,
        }
    }

    pub(crate) fn kind(&self) -> &Kind {
        &self.inner.kind
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("modio::Error");

        builder.field("kind", &self.inner.kind);

        if let Some(ref source) = self.inner.source {
            builder.field("source", source);
        }
        builder.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.kind {
            Kind::Auth(ref err) => write!(f, "authentication error: {err}")?,
            Kind::Builder => f.write_str("builder error")?,
            Kind::Decode => f.write_str("error decoding response body")?,
            Kind::Download => f.write_str("download error")?,
            Kind::Request => f.write_str("http request error")?,
            Kind::Status(code) => {
                let prefix = if code.is_client_error() {
                    "HTTP status client error"
                } else {
                    debug_assert!(code.is_server_error());
                    "HTTP status server error"
                };
                write!(f, "{prefix} ({code})")?;
            }
            Kind::RateLimit { retry_after } => {
                write!(f, "API rate limit reached. Try again in {retry_after:?}.")?;
            }
            Kind::Validation {
                ref message,
                ref errors,
            } => {
                write!(f, "validation failed: '{message}' {errors:?}")?;
            }
        };
        if let Some(ref e) = self.inner.source {
            write!(f, ": {e}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.inner.source.as_ref().map(|e| &**e as _)
    }
}

#[derive(Debug)]
pub(crate) enum Kind {
    Auth(AuthError),
    Download,
    Validation {
        message: String,
        errors: Vec<(String, String)>,
    },
    RateLimit {
        retry_after: Duration,
    },
    Builder,
    Request,
    Decode,
    Status(StatusCode),
}

impl StdError for ModioError {}

impl fmt::Display for ModioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = String::new();
        buf.push_str(&self.message);
        for (k, v) in &self.errors {
            buf.push('\n');
            buf.push_str("  ");
            buf.push_str(k);
            buf.push_str(": ");
            buf.push_str(v);
        }
        fmt::Display::fmt(&buf, f)
    }
}

pub(crate) fn token_required() -> Error {
    Error::new(Kind::Auth(AuthError::TokenRequired)).with(AuthError::TokenRequired)
}

pub(crate) fn unauthorized(error_ref: u16) -> Error {
    Error::new(Kind::Auth(AuthError::Unauthorized))
        .with_error_ref(error_ref)
        .with(AuthError::Unauthorized)
}

pub(crate) fn terms_required() -> Error {
    Error::new(Kind::Auth(AuthError::TermsAcceptanceRequired))
        .with(AuthError::TermsAcceptanceRequired)
}

pub(crate) fn builder_or_request(e: reqwest::Error) -> Error {
    if e.is_builder() {
        builder(e)
    } else {
        request(e)
    }
}

pub(crate) fn builder<E: Into<BoxError>>(source: E) -> Error {
    Error::new(Kind::Builder).with(source)
}

pub(crate) fn request<E: Into<BoxError>>(source: E) -> Error {
    Error::new(Kind::Request).with(source)
}

pub(crate) fn decode<E: Into<BoxError>>(source: E) -> Error {
    Error::new(Kind::Decode).with(source)
}

pub(crate) fn error_for_status(status: StatusCode, error: ModioError) -> Error {
    match status {
        StatusCode::UNPROCESSABLE_ENTITY => Error::new(Kind::Validation {
            message: error.message,
            errors: error.errors,
        })
        .with_error_ref(error.error_ref),
        StatusCode::UNAUTHORIZED => unauthorized(error.error_ref),
        StatusCode::FORBIDDEN if error.error_ref == 11051 => terms_required(),
        _ => Error::new(Kind::Status(status))
            .with_error_ref(error.error_ref)
            .with(error),
    }
}

pub(crate) fn ratelimit(retry_after: u64) -> Error {
    Error::new(Kind::RateLimit {
        retry_after: Duration::from_secs(retry_after),
    })
}

pub(crate) fn download<E: Into<BoxError>>(source: E) -> Error {
    Error::new(Kind::Download).with(source)
}
