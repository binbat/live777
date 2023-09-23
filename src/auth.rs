use std::{collections::HashSet, marker::PhantomData};

use crate::config::Auth;
use base64::{engine::general_purpose::STANDARD, Engine};
use http::{header, Request, Response, StatusCode};
use http_body::Body;
use tower_http::validate_request::ValidateRequest;

#[derive(Debug)]
pub struct ManyValidate<ResBody> {
    header_values: HashSet<String>,
    _ty: PhantomData<fn() -> ResBody>,
}

impl<ResBody> ManyValidate<ResBody> {
    pub fn new(auth: Auth) -> Self
    where
        ResBody: Body + Default,
    {
        let mut header_values = HashSet::new();
        for account in auth.accounts {
            let encoded = STANDARD.encode(format!("{}:{}", account.username, account.password));
            header_values.insert(format!("Basic {}", encoded).parse().unwrap());
        }
        for token in auth.tokens {
            header_values.insert(format!("Bearer {}", token).parse().unwrap());
        }
        Self {
            header_values,
            _ty: PhantomData,
        }
    }
}

impl<ResBody> Clone for ManyValidate<ResBody> {
    fn clone(&self) -> Self {
        Self {
            header_values: self.header_values.clone(),
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
        if self.header_values.is_empty() {
            return Ok(());
        }
        match request.headers().get(header::AUTHORIZATION) {
            Some(actual) if self.header_values.contains(actual.to_str().unwrap()) => Ok(()),
            _ => {
                let mut res = Response::new(ResBody::default());
                *res.status_mut() = StatusCode::UNAUTHORIZED;
                Err(res)
            }
        }
    }
}
