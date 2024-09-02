use std::time::{Duration, SystemTime};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error};

use auth::Keys;

use crate::AppState;

const JWT_TOKEN_EXPIRES: Duration = Duration::from_secs(60 * 60 * 24);

pub async fn authorize(
    State(state): State<AppState>,
    Json(payload): Json<AuthPayload>,
) -> Result<Json<AuthBody>, AuthError> {
    // Check if the user sent the credentials
    if payload.username.is_empty() || payload.password.is_empty() {
        return Err(AuthError::MissingCredentials);
    }
    // Here you can check the user credentials from a database
    let mut user: Option<usize> = None;
    for (i, account) in state.config.auth.admin.iter().enumerate() {
        if payload.username == account.username && payload.password == account.password {
            user = Some(i);
        }
    }

    if user.is_none() {
        return Err(AuthError::WrongCredentials);
    }

    debug!("User UID: {:?}", user);

    let keys = Keys::new(state.config.auth.secret.as_bytes());
    let token = keys
        .token(
            user.unwrap_or(0),
            (SystemTime::now() + JWT_TOKEN_EXPIRES)
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as usize,
            7,
        )
        .map_err(|err| {
            error!("Error while encoding: {err}");
            AuthError::TokenCreation
        })?;

    // Send the authorized token
    Ok(Json(AuthBody::new(token)))
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AuthError::WrongCredentials => (StatusCode::UNAUTHORIZED, "Wrong credentials"),
            AuthError::MissingCredentials => (StatusCode::BAD_REQUEST, "Missing credentials"),
            AuthError::TokenCreation => (StatusCode::INTERNAL_SERVER_ERROR, "Token creation error"),
        };
        let body = Json(json!({
            "error": error_message,
        }));
        (status, body).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthPayload {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthBody {
    access_token: String,
    token_type: String,
}

impl AuthBody {
    fn new(access_token: String) -> Self {
        Self {
            access_token,
            token_type: "Bearer".to_string(),
        }
    }
}

#[derive(Debug)]
pub enum AuthError {
    WrongCredentials,
    MissingCredentials,
    TokenCreation,
}
