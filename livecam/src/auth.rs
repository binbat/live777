use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router, async_trait,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::post,
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::error;

use super::config::Config;
use super::{LiveCamManager, utils};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    pub manager: LiveCamManager,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

#[derive(Deserialize)]
pub struct LoginPayload {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordPayload {
    new_password: String,
}

async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    let config = state.config.read().unwrap();

    let parsed_hash = match PasswordHash::new(&config.auth.password_hash) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server configuration error",
            )
                .into_response();
        }
    };

    if Argon2::default()
        .verify_password(payload.password.as_bytes(), &parsed_hash)
        .is_ok()
    {
        let now = chrono::Utc::now();
        let exp = (now + chrono::Duration::hours(24)).timestamp() as usize;
        let claims = Claims {
            sub: payload.username.clone(),
            exp,
        };

        let token = match encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(config.auth.jwt_secret.as_ref()),
        ) {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to create JWT token: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to create token")
                    .into_response();
            }
        };
        (StatusCode::OK, Json(serde_json::json!({ "token": token }))).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, "Invalid credentials").into_response()
    }
}

async fn change_password_handler(
    State(state): State<AppState>,
    _claims: Claims,
    Json(payload): Json<ChangePasswordPayload>,
) -> impl IntoResponse {
    let salt = SaltString::generate(&mut OsRng);
    let new_password_hash =
        match Argon2::default().hash_password(payload.new_password.as_bytes(), &salt) {
            Ok(hash) => hash.to_string(),
            Err(e) => {
                error!("Failed to hash new password: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to process password",
                )
                    .into_response();
            }
        };

    {
        let mut config_guard = state.config.write().unwrap();
        config_guard.auth.password_hash = new_password_hash;

        if let Err(e) = utils::save_config("livecam", &*config_guard) {
            error!("Failed to save updated config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to save configuration",
            )
                .into_response();
        }
    }

    (StatusCode::OK, "Password updated successfully").into_response()
}

async fn reset_config_handler(_claims: Claims) -> impl IntoResponse {
    if let Err(e) = super::utils::reset_config("livecam") {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to reset config: {}", e),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        "Configuration reset. Please restart the application to apply changes.",
    )
        .into_response()
}

#[async_trait]
impl FromRequestParts<AppState> for Claims {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let config = state.config.read().unwrap();
        let secret = config.auth.jwt_secret.clone();

        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.starts_with("Bearer "));

        if let Some(auth_header) = auth_header {
            let token = &auth_header[7..];
            let decoding_key = DecodingKey::from_secret(secret.as_ref());
            let validation = Validation::new(jsonwebtoken::Algorithm::HS256);

            match decode::<Claims>(token, &decoding_key, &validation) {
                Ok(token_data) => Ok(token_data.claims),
                Err(_) => Err((StatusCode::UNAUTHORIZED, "Invalid token").into_response()),
            }
        } else {
            Err((StatusCode::UNAUTHORIZED, "Missing authentication token").into_response())
        }
    }
}

pub fn create_auth_router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/api/login", post(login_handler))
        .route("/api/user/password", post(change_password_handler))
        .route("/api/config/reset", post(reset_config_handler))
        .route("/api/session", post(session_handler))
}

async fn session_handler(_claims: Claims) -> impl IntoResponse {
    (StatusCode::OK, "Session is valid")
}
