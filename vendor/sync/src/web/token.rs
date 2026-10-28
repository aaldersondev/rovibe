//! RoVibe: the serve token. When the app starts a sync server it hands it a
//! token in `ROVIBE_SYNC_TOKEN` and gives the same token to the Studio plugin;
//! every API request must then carry it in `X-RoVibe-Token`, the WebSocket
//! upgrade included (Studio sends the `Headers` of `CreateWebStreamClient`).
//! Another local program can no longer read the project or write into it.
//! Without the variable, as when someone runs `rojo serve` by hand, nothing
//! changes.

use hyper::{Body, Request, Response, StatusCode};

use super::{interface::ErrorResponse, util::msgpack};

pub const HEADER: &str = "x-rovibe-token";

/// The token this server requires, read once at start.
pub fn from_env() -> Option<String> {
    std::env::var("ROVIBE_SYNC_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
}

/// A response refusing `request`, or `None` when it may go through.
pub fn check(request: &Request<Body>, required: Option<&str>) -> Option<Response<Body>> {
    let required = required?;
    let given = request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok());
    if given.is_some_and(|given| constant_time_eq(given.as_bytes(), required.as_bytes())) {
        return None;
    }
    Some(msgpack(
        ErrorResponse::unauthorized(
            "This sync server is managed by RoVibe and needs its token: connect from the RoVibe app.",
        ),
        StatusCode::UNAUTHORIZED,
    ))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/api/rojo");
        if let Some(token) = token {
            builder = builder.header(HEADER, token);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn without_a_required_token_everything_goes_through() {
        assert!(check(&request(None), None).is_none());
    }

    #[test]
    fn the_right_token_goes_through_and_anything_else_is_refused() {
        assert!(check(&request(Some("s3cret")), Some("s3cret")).is_none());
        let refused = check(&request(Some("wrong")), Some("s3cret")).unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert!(check(&request(None), Some("s3cret")).is_some());
        assert!(check(&request(Some("s3cre")), Some("s3cret")).is_some());
    }
}
