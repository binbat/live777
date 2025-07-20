use std::collections::HashSet;

use anyhow::{anyhow, Error};
use headers::authorization::{Bearer, Credentials};
use http::{header, StatusCode};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::claims::Claims;

pub mod access;
pub mod claims;

pub const ANY_ID: &str = "*";

pub struct Keys {
    encoding: EncodingKey,
}

impl Keys {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret),
        }
    }

    pub fn token(self, claims: Claims) -> Result<String, Error> {
        encode(&Header::default(), &claims, &self.encoding).map_err(|e| anyhow!(e))
    }
}

#[derive(Clone)]
pub struct AuthState {
    tokens: HashSet<String>,
    decoding: DecodingKey,
}

impl AuthState {
    pub fn new(secret: String, tokens: Vec<String>) -> Self {
        Self {
            tokens: tokens.into_iter().collect(),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        }
    }
}

pub async fn validate_middleware(
    State(state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut closure = || {
        if state.tokens.is_empty() {
            request.extensions_mut().insert(Claims {
                id: ANY_ID.to_string(),
                exp: 0,
                mode: 7,
            });
            return true;
        }

        if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
            match Bearer::decode(auth_header) {
                Some(bearer) if state.tokens.contains(bearer.token()) => {
                    request.extensions_mut().insert(Claims {
                        id: ANY_ID.to_string(),
                        exp: 0,
                        mode: 7,
                    });
                    return true;
                }
                Some(bearer) => {
                    if let Ok(token_data) =
                        decode::<Claims>(bearer.token(), &state.decoding, &Validation::default())
                    {
                        request.extensions_mut().insert(token_data.claims);
                        return true;
                    }
                }
                _ => (),
            }
        };
        false
    };

    if closure() {
        next.run(request).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(axum::body::Body::default())
            .unwrap()
    }
}
