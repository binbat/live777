use std::{collections::HashSet, marker::PhantomData};

use anyhow::{anyhow, Error};
use headers::authorization::{Bearer, Credentials};
use http::{header, Request, Response, StatusCode};
use http_body::Body;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use tower_http::validate_request::ValidateRequest;

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

    pub fn token(self, id: String, exp: usize, mode: u8) -> Result<String, Error> {
        encode(
            &Header::default(),
            &Claims { id, exp, mode },
            &self.encoding,
        )
        .map_err(|e| anyhow!(e))
    }
}

pub struct ManyValidate<ResBody> {
    tokens: HashSet<String>,
    decoding: DecodingKey,
    _ty: PhantomData<fn() -> ResBody>,
}

impl<ResBody> ManyValidate<ResBody> {
    pub fn new(secret: String, tokens: Vec<String>) -> Self
    where
        ResBody: Body + Default,
    {
        Self {
            tokens: tokens.into_iter().collect(),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
            _ty: PhantomData,
        }
    }
}

impl<ResBody> Clone for ManyValidate<ResBody> {
    fn clone(&self) -> Self {
        Self {
            tokens: self.tokens.clone(),
            decoding: self.decoding.clone(),
            _ty: PhantomData,
        }
    }
}

impl<B, ResBody> ValidateRequest<B> for ManyValidate<ResBody>
where
    ResBody: Body + Default,
{
    type ResponseBody = ResBody;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<Self::ResponseBody>> {
        if self.tokens.is_empty() {
            request.extensions_mut().insert(Claims {
                id: ANY_ID.to_string(),
                exp: 0,
                mode: 7,
            });
            return Ok(());
        }
        (match request.headers().get(header::AUTHORIZATION) {
            Some(auth_header) => match Bearer::decode(auth_header) {
                Some(bearer) if self.tokens.contains(bearer.token()) => {
                    // Static token is max permissions
                    request.extensions_mut().insert(Claims {
                        id: ANY_ID.to_string(),
                        exp: 0,
                        mode: 7,
                    });
                    Ok(())
                }
                Some(bearer) => {
                    match decode::<Claims>(bearer.token(), &self.decoding, &Validation::default()) {
                        Ok(token_data) => {
                            request.extensions_mut().insert(token_data.claims);
                            Ok(())
                        }
                        _ => Err(()),
                    }
                }
                _ => Err(()),
            },
            _ => Err(()),
        })
        .map_err(|_| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(ResBody::default())
                .unwrap()
        })
    }
}
