use std::collections::HashSet;

use anyhow::Error;
use headers::authorization::{Bearer, Credentials};
use http::{StatusCode, header};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::claims::Claims;

pub mod access;
pub mod claims;
mod jwt;

pub const ANY_ID: &str = "*";

pub struct Keys {
    secret: Vec<u8>,
}

impl Keys {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            secret: secret.to_vec(),
        }
    }

    pub fn token(self, claims: Claims) -> Result<String, Error> {
        jwt::encode(&claims, &self.secret)
    }
}

#[derive(Clone)]
pub struct AuthState {
    tokens: HashSet<String>,
    secret: Vec<u8>,
}

impl AuthState {
    pub fn new(secret: String, tokens: Vec<String>) -> Self {
        Self {
            tokens: tokens.into_iter().collect(),
            secret: secret.into_bytes(),
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
                    if let Ok(claims) = jwt::decode(bearer.token(), &state.secret) {
                        request.extensions_mut().insert(claims);
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
