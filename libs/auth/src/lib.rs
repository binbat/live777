use std::{collections::HashSet, marker::PhantomData};

use headers::authorization::{Bearer, Credentials};
use http::{header, Request, Response, StatusCode};
use http_body::Body;
use tower_http::validate_request::ValidateRequest;

mod claim;

#[derive(Debug)]
pub struct ManyValidate<ResBody> {
    tokens: HashSet<String>,
    _ty: PhantomData<fn() -> ResBody>,
}

impl<ResBody> ManyValidate<ResBody> {
    pub fn new(tokens: Vec<String>) -> Self
    where
        ResBody: Body + Default,
    {
        Self {
            tokens: tokens.into_iter().collect(),
            _ty: PhantomData,
        }
    }
}

impl<ResBody> Clone for ManyValidate<ResBody> {
    fn clone(&self) -> Self {
        Self {
            tokens: self.tokens.clone(),
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
            return Ok(());
        }
        (match request.headers().get(header::AUTHORIZATION) {
            Some(auth_header) => match Bearer::decode(auth_header) {
                Some(bearer) if self.tokens.contains(bearer.token()) => Ok(()),
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
