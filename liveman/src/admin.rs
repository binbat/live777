use std::marker::PhantomData;
use std::time::{Duration, SystemTime};
use std::{fmt::Display, usize};

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::headers::authorization::{Bearer, Credentials};
use http_body::Body;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::validate_request::ValidateRequest;
use tracing::{debug, error};

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

    let claims = Claims {
        uid: user.unwrap_or(0),
        exp: (SystemTime::now() + JWT_TOKEN_EXPIRES)
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize,
    };

    let keys = Keys::new(state.config.auth.secret.as_bytes());
    // Create the authorization token
    let token = encode(&Header::default(), &claims, &keys.encoding)
        .map_err(|_| AuthError::TokenCreation)?;

    // Send the authorized token
    Ok(Json(AuthBody::new(token)))
}

#[derive(Debug)]
pub struct JWTValidate<ResBody> {
    secret: String,
    noauth: bool,
    _ty: PhantomData<fn() -> ResBody>,
}

impl<ResBody> Clone for JWTValidate<ResBody> {
    fn clone(&self) -> Self {
        Self {
            secret: self.secret.clone(),
            noauth: self.noauth,
            _ty: PhantomData,
        }
    }
}

impl<ResBody> JWTValidate<ResBody> {
    pub fn new(secret: String, noauth: bool) -> Self
    where
        ResBody: Body + Default,
    {
        Self {
            secret,
            noauth,
            _ty: PhantomData,
        }
    }
}

impl<B, ResBody> ValidateRequest<B> for JWTValidate<ResBody>
where
    ResBody: Body + Default,
{
    type ResponseBody = ResBody;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<Self::ResponseBody>> {
        match request.headers().get(header::AUTHORIZATION) {
            Some(auth_header) => match Bearer::decode(auth_header) {
                Some(bearer) => {
                    let keys = Keys::new(self.secret.as_bytes());
                    match decode::<Claims>(bearer.token(), &keys.decoding, &Validation::default()) {
                        Ok(token_data) => {
                            request.extensions_mut().insert(token_data.claims);
                        }
                        Err(err) => error!("Error while decoding: {err}"),
                    };
                }
                None => debug!("No Authorization header bearer found"),
            },
            None => debug!("No Authorization header found"),
        };

        match request.extensions().get::<Claims>() {
            Some(_) => Ok(()),
            None => {
                if self.noauth {
                    Ok(())
                } else {
                    Err(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(ResBody::default())
                        .unwrap())
                }
            }
        }
    }
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

impl Display for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Email: {}\nCompany: {}", self.uid, self.exp)
    }
}

impl AuthBody {
    fn new(access_token: String) -> Self {
        Self {
            access_token,
            token_type: "Bearer".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    uid: usize,
    exp: usize,
}

struct Keys {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl Keys {
    fn new(secret: &[u8]) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
        }
    }
}

#[derive(Debug)]
pub enum AuthError {
    WrongCredentials,
    MissingCredentials,
    TokenCreation,
}
