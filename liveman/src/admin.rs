use std::time::{Duration, SystemTime};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error};

use auth::{
    ANY_ID, Keys,
    claims::{Access, Claims},
};

use crate::{AppState, config::Account};

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
    let mut user: Option<&Account> = None;
    for account in state.config.auth.accounts.iter() {
        if payload.username == account.username && payload.password == account.password {
            user = Some(account);
        }
    }

    if user.is_none() {
        return Err(AuthError::WrongCredentials);
    }

    debug!("User UID: {:?}", user);

    let keys = Keys::new(state.config.auth.secret.as_bytes());
    let token = keys
        .token(Claims {
            id: ANY_ID.to_string(),
            exp: (SystemTime::now() + JWT_TOKEN_EXPIRES)
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            mode: 7,
        })
        .map_err(|err| {
            error!("Error while encoding: {err}");
            AuthError::TokenCreation
        })?;

    // Send the authorized token
    Ok(Json(AuthBody::new(token)))
}

pub async fn token(
    State(state): State<AppState>,
    Json(payload): Json<TokenPayload>,
) -> Result<Json<AuthBody>, AuthError> {
    let keys = Keys::new(state.config.auth.secret.as_bytes());
    let token = keys.token(payload.into()).map_err(|err| {
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
pub struct TokenPayload {
    id: String,
    duration: u64,
    subscribe: bool,
    publish: bool,
    admin: bool,
}

impl From<TokenPayload> for Claims {
    fn from(v: TokenPayload) -> Self {
        Self {
            id: v.id,
            exp: (SystemTime::now() + Duration::from_secs(v.duration))
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            mode: (Access {
                r: v.subscribe,
                w: v.publish,
                x: v.admin,
            })
            .into(),
        }
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
