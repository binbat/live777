use axum::response::{IntoResponse, Response};
use http::StatusCode;

#[derive(Debug)]
pub enum AppError {
    NoAvailableNode,
    RequestProxyError,
    ResourceNotFound,
    InternalServerError(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::InternalServerError(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
            }
            AppError::RequestProxyError => {
                (StatusCode::BAD_REQUEST, "reqeust error".to_string()).into_response()
            }
            AppError::NoAvailableNode => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no available node".to_string(),
            )
                .into_response(),
            AppError::ResourceNotFound => {
                (StatusCode::NOT_FOUND, "resource not exists".to_string()).into_response()
            }
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
