use axum::response::{IntoResponse, Response};
use http::StatusCode;

#[derive(Debug)]
pub enum AppError {
    ResourceNotFound(String),
    ResourceAlreadyExists(String),
    Throw(String),
    InternalServerError(anyhow::Error),
}

impl AppError {
    pub fn resource_not_fount<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::ResourceNotFound(t.to_string())
    }

    pub fn resource_already_exists<T>(t: T) -> Self
    where
        T: ToString,
    {
        AppError::ResourceAlreadyExists(t.to_string())
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
            AppError::ResourceNotFound(err) => (StatusCode::NOT_FOUND, err).into_response(),
            AppError::ResourceAlreadyExists(err) => (StatusCode::CONFLICT, err).into_response(),
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
