use axum::{extract::Request, http, middleware::Next, response::Response};
use http::method::Method;

use crate::{
    ANY_ID,
    claims::{Access, Claims},
};

pub async fn access_middleware(request: Request, next: Next) -> Response {
    let ok = match request.extensions().get::<Claims>() {
        Some(claims) => allowed(claims, request.method(), request.uri().path()),
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

fn allowed(claims: &Claims, method: &Method, path: &str) -> bool {
    match (claims.id.as_str(), method, path) {
        (id, &Method::GET, path) if path == api::path::streams(id) => true,
        // Allow Liveman auto-record APIs for JWT with specific id
        (id, method, path) if path == api::path::record(id) => match *method {
            Method::POST => Access::from(claims.mode).x,
            Method::GET => Access::from(claims.mode).r,
            Method::DELETE => Access::from(claims.mode).x,
            _ => false,
        },
        (id, &Method::DELETE, path) if path == api::path::streams(id) => {
            Access::from(claims.mode).x
        }
        (id, &Method::POST, path) if path == api::path::whip(id) => Access::from(claims.mode).w,
        (id, &Method::POST, path) if path == api::path::whep(id) => Access::from(claims.mode).r,
        (id, &Method::POST, path) if path == api::path::cascade(id) => Access::from(claims.mode).x,
        // Cable messages: both directions need the `x` scope. POST
        // injects into peers; the GET exposes every peer's messages
        // towards the publisher — traffic a plain WHEP subscriber
        // (scope `r`) could never see — so read access is an
        // operator/API-server capability too, not a viewer one.
        (id, method, path)
            if path.starts_with(&format!("{}/cables/", api::path::streams(id)))
                && path.ends_with("/messages") =>
        {
            match *method {
                Method::POST => Access::from(claims.mode).x,
                Method::GET => Access::from(claims.mode).x,
                _ => false,
            }
        }
        (id, &Method::POST, path) if path == api::path::targets(id) => Access::from(claims.mode).x,
        (id, &Method::DELETE, path) if path == api::path::targets(id) => {
            Access::from(claims.mode).x
        }
        (id, &Method::GET, path) if path == api::path::targets(id) => Access::from(claims.mode).r,
        // Playback entries are per-stream: a stream-scoped token may list
        // its own stream's recordings. The all-streams list stays
        // wildcard-only.
        (id, &Method::GET, path) if path == format!("/api/playback/{id}") => {
            Access::from(claims.mode).r
        }
        // Recording objects live under `{stream}/{ts}/...`, so a
        // stream-scoped token may read its own stream's objects. With a
        // custom `base_dir` the first path segment is not the stream name
        // and only a wildcard token passes.
        (id, &Method::GET, path)
            if path
                .strip_prefix("/api/record/object/")
                .and_then(|rest| rest.split('/').next())
                == Some(id) =>
        {
            Access::from(claims.mode).r
        }
        (id, _, _) if id == ANY_ID => true,
        (id, &Method::POST, path) if path == "/token" && id == ANY_ID => {
            Access::from(claims.mode).r
                && Access::from(claims.mode).w
                && Access::from(claims.mode).x
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u8 = 4;
    const X: u8 = 1;

    fn claims(id: &str, mode: u8) -> Claims {
        Claims {
            id: id.to_string(),
            exp: 0,
            mode,
        }
    }

    #[test]
    fn playback_entries_allow_stream_scoped_read() {
        assert!(allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/playback/cam1"
        ));
        assert!(!allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/playback/cam2"
        ));
        // The all-streams list stays wildcard-only.
        assert!(!allowed(&claims("cam1", R), &Method::GET, "/api/playback"));
        // The read scope is required.
        assert!(!allowed(
            &claims("cam1", 0),
            &Method::GET,
            "/api/playback/cam1"
        ));
    }

    #[test]
    fn record_objects_allow_stream_scoped_read() {
        assert!(allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/record/object/cam1/1/manifest.mpd"
        ));
        assert!(allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/record/object/cam1/1/v_init.m4s"
        ));
        assert!(!allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/record/object/cam2/1/manifest.mpd"
        ));
        // With a custom base_dir the first path segment is not the stream
        // name, so a stream-scoped token no longer matches.
        assert!(!allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/record/object/web-0/2025/05/05/manifest.mpd"
        ));
        // Writes stay out of reach even for a full-scope stream token.
        assert!(!allowed(
            &claims("cam1", 7),
            &Method::DELETE,
            "/api/record/object/cam1/1/manifest.mpd"
        ));
    }

    #[test]
    fn wildcard_token_keeps_full_access() {
        assert!(allowed(
            &claims(ANY_ID, 0),
            &Method::GET,
            "/api/playback/cam1"
        ));
        assert!(allowed(&claims(ANY_ID, 0), &Method::GET, "/api/playback"));
        assert!(allowed(
            &claims(ANY_ID, 0),
            &Method::GET,
            "/api/record/object/cam1/1/manifest.mpd"
        ));
        assert!(allowed(
            &claims(ANY_ID, 0),
            &Method::DELETE,
            "/api/recordings"
        ));
    }

    #[test]
    fn existing_per_id_rules_unchanged() {
        // Record start: POST/DELETE need x, GET needs r.
        assert!(allowed(
            &claims("cam1", X),
            &Method::POST,
            "/api/record/cam1"
        ));
        assert!(!allowed(
            &claims("cam1", R),
            &Method::POST,
            "/api/record/cam1"
        ));
        assert!(allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/record/cam1"
        ));
        assert!(allowed(
            &claims("cam1", X),
            &Method::DELETE,
            "/api/record/cam1"
        ));
        // The recordings index sync surface stays wildcard-only.
        assert!(!allowed(
            &claims("cam1", R),
            &Method::GET,
            "/api/recordings"
        ));
        assert!(!allowed(
            &claims("cam1", X),
            &Method::DELETE,
            "/api/recordings"
        ));
        // WHIP publish needs w, WHEP subscribe needs r.
        assert!(allowed(&claims("cam1", 2), &Method::POST, "/whip/cam1"));
        assert!(allowed(&claims("cam1", R), &Method::POST, "/whep/cam1"));
        assert!(!allowed(&claims("cam1", X), &Method::POST, "/whep/cam1"));
    }
}
