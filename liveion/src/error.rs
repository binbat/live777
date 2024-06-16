use axum::response::{IntoResponse, Response};
use http::StatusCode;

#[derive(Debug)]
pub enum AppError {
    StreamNotFound(String),
    StreamAlreadyExists(String),
    SessionNotFound(String),
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
            AppError::SessionNotFound(err) => (StatusCode::NOT_FOUND, err).into_response(),
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
