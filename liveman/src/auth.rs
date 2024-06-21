use std::{collections::HashSet, marker::PhantomData};

use http::{header, Request, Response, StatusCode};
use http_body::Body;
use tower_http::validate_request::ValidateRequest;

use crate::config::Auth;

#[derive(Debug)]
pub struct ManyValidate<ResBody> {
    header_values: HashSet<String>,
    _ty: PhantomData<fn() -> ResBody>,
}

impl<ResBody> ManyValidate<ResBody> {
    pub fn new(auths: Vec<Auth>) -> Self
    where
        ResBody: Body + Default,
    {
        let mut header_values = HashSet::new();
        for auth in auths {
            for authorization in auth.to_authorizations().into_iter() {
                header_values.insert(authorization.parse().unwrap());
            }
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
