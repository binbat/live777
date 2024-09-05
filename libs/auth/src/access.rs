use axum::{extract::Request, http, middleware::Next, response::Response};
use http::method::Method;

use crate::{
    claims::{Access, Claims},
    ANY_ID,
};

pub async fn access_middleware(request: Request, next: Next) -> Response {
    let ok = match request.extensions().get::<Claims>() {
        Some(claims) => match (claims.id.clone(), request.method(), request.uri().path()) {
            (id, &Method::GET, path) if path == api::path::streams(&id) => true,
            (id, &Method::DELETE, path) if path == api::path::streams(&id) => {
                Access::from(claims.mode).x
            }
            (id, &Method::POST, path) if path == api::path::whip(&id) => {
                Access::from(claims.mode).w
            }
            (id, &Method::POST, path) if path == api::path::whep(&id) => {
                Access::from(claims.mode).r
            }
            (id, &Method::POST, path) if path == api::path::cascade(&id) => {
                Access::from(claims.mode).x
            }
            (id, _, _) if id == ANY_ID => true,
            _ => false,
        },
        None => false,
    };

    if !ok {
        return Response::builder()
            .status(http::StatusCode::FORBIDDEN)
            .body("Don't permission".into())
            .unwrap();
    }

    next.run(request).await
}
