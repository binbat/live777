use axum::response::{IntoResponse, Response};
use http::StatusCode;

#[derive(Debug)]
pub enum AppError {
    StreamNotFound(String),
    StreamAlreadyExists(String),
    /// Operation rejected because the stream is declared in the config file
    /// (provisioned): it cannot be deleted or recreated through the API.
    StreamProvisioned(String),
    /// Publish rejected because the stream's configured source is already
    /// feeding tracks — a second publisher would mix both publishers'
    /// tracks into every subscriber.
    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    StreamSourceActive(String),
    SessionNotFound(String),
    /// No media source is registered for the requested stream.
    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    SourceNotFound(String),
    /// The stream's encoder rejected a runtime bitrate change (fixed-bitrate
    /// backend or pipeline not running).
    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    SourceBitrateUnsupported(String),
    /// The request itself is malformed (semantically); maps to 400.
    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    BadRequest(String),
    Throw(String),
    InternalServerError(anyhow::Error),
}

impl AppError {
    pub fn stream_not_found<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::StreamNotFound(t.to_string())
    }

    pub fn stream_already_exists<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::StreamAlreadyExists(t.to_string())
    }

    pub fn session_not_found<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::SessionNotFound(t.to_string())
    }

    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    pub fn stream_source_active<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::StreamSourceActive(t.to_string())
    }

    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    pub fn source_not_found<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::SourceNotFound(t.to_string())
    }

    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    pub fn source_bitrate_unsupported<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::SourceBitrateUnsupported(t.to_string())
    }

    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    pub fn bad_request<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::BadRequest(t.to_string())
    }

    pub fn throw<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::Throw(t.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::StreamNotFound(err) => (StatusCode::NOT_FOUND, err).into_response(),
            AppError::StreamAlreadyExists(err) => (StatusCode::CONFLICT, err).into_response(),
            AppError::StreamProvisioned(err) => (StatusCode::CONFLICT, err).into_response(),
            AppError::StreamSourceActive(err) => (StatusCode::CONFLICT, err).into_response(),
            AppError::SessionNotFound(err) => (StatusCode::NOT_FOUND, err).into_response(),
            AppError::SourceNotFound(err) => (StatusCode::NOT_FOUND, err).into_response(),
            AppError::SourceBitrateUnsupported(err) => (StatusCode::CONFLICT, err).into_response(),
            AppError::BadRequest(err) => (StatusCode::BAD_REQUEST, err).into_response(),
            AppError::InternalServerError(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
            }
            AppError::Throw(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        AppError::InternalServerError(err.into())
    }
}
