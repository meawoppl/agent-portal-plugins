//! Serving the embedded Yew frontend.
//!
//! `backend/build.rs` turns `frontend/dist` into a `&'static [EmbeddedAsset]`
//! expression that the `kicadmium` binary includes and hands to [`crate::run`].
//! Everything is pre-compressed with brotli at build time; this module only
//! negotiates encodings, answers conditional requests and falls back to the
//! single-page entry point.

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};

/// One file from `frontend/dist`, embedded in the binary.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedAsset {
    /// Request path, e.g. `/index.html` or `/kicad-viewer/three/three.module.js`.
    pub route: &'static str,
    pub content_type: &'static str,
    /// Strong validator derived from the content, already quoted.
    pub etag: &'static str,
    pub bytes: &'static [u8],
    /// Brotli-compressed `bytes`, when that was smaller.
    pub brotli: Option<&'static [u8]>,
}

const INDEX: &str = "/index.html";
/// HTML revalidates on every load (its hashed asset names change per build);
/// hashed assets are kept for a week and refreshed in the background.
const HTML_CACHE: &str = "no-cache";
const ASSET_CACHE: &str = "max-age=604800, stale-while-revalidate=86400";

/// Router serving `assets` for every path that reaches it, with `/index.html`
/// as the single-page fallback (status 200) for anything unknown.
pub(crate) fn router(assets: &'static [EmbeddedAsset]) -> Router {
    Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap| async move {
            serve(assets, &method, uri.path(), &headers)
        },
    )
}

fn lookup<'a>(assets: &'a [EmbeddedAsset], path: &str) -> Option<&'a EmbeddedAsset> {
    let path = if path == "/" { INDEX } else { path };
    assets.iter().find(|asset| asset.route == path)
}

fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|encoding| encoding.trim().split(';').next() == Some("br"))
        })
}

fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        })
}

pub(crate) fn serve(
    assets: &'static [EmbeddedAsset],
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(asset) = lookup(assets, path).or_else(|| lookup(assets, INDEX)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let html = asset.content_type.starts_with("text/html");
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::ETAG, asset.etag)
        .header(
            header::CACHE_CONTROL,
            if html { HTML_CACHE } else { ASSET_CACHE },
        );
    if asset.brotli.is_some() {
        response = response.header(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    if matches_etag(headers, asset.etag) {
        return response
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .expect("static headers are valid");
    }
    let body = match asset.brotli {
        Some(compressed) if accepts_brotli(headers) => {
            response = response.header(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
            compressed
        }
        _ => asset.bytes,
    };
    response = response.header(header::CONTENT_LENGTH, body.len());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    };
    response.body(body).expect("static headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    static ASSETS: &[EmbeddedAsset] = &[
        EmbeddedAsset {
            route: "/index.html",
            content_type: "text/html",
            etag: "\"index-1\"",
            bytes: b"<html>app</html>",
            brotli: None,
        },
        EmbeddedAsset {
            route: "/app-abc123.js",
            content_type: "text/javascript",
            etag: "\"js-1\"",
            bytes: b"console.log('raw');",
            brotli: Some(b"br-bytes"),
        },
    ];

    async fn get(path: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::get(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = router(ASSETS)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    #[tokio::test]
    async fn root_and_unknown_paths_serve_the_spa_entry() {
        for path in ["/", "/index.html", "/pcb", "/some/deep/route"] {
            let (status, headers, body) = get(path, &[]).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(body, b"<html>app</html>", "{path}");
            assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
            assert_eq!(headers[header::ETAG], "\"index-1\"");
            assert!(headers.get(header::VARY).is_none());
        }
    }

    #[tokio::test]
    async fn hashed_assets_cache_and_negotiate_brotli() {
        let (status, headers, body) = get("/app-abc123.js", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"console.log('raw');");
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript");
        assert_eq!(
            headers[header::CACHE_CONTROL],
            "max-age=604800, stale-while-revalidate=86400"
        );
        assert_eq!(headers[header::VARY], "accept-encoding");
        assert!(headers.get(header::CONTENT_ENCODING).is_none());

        let (_, headers, body) = get(
            "/app-abc123.js",
            &[("accept-encoding", "gzip, deflate, br;q=0.9")],
        )
        .await;
        assert_eq!(headers[header::CONTENT_ENCODING], "br");
        assert_eq!(headers[header::CONTENT_LENGTH], "8");
        assert_eq!(body, b"br-bytes");
    }

    #[tokio::test]
    async fn conditional_requests_and_methods() {
        let (status, headers, body) = get("/app-abc123.js", &[("if-none-match", "\"js-1\"")]).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert_eq!(headers[header::ETAG], "\"js-1\"");
        assert!(body.is_empty());

        let (status, _, _) = get("/app-abc123.js", &[("if-none-match", "\"stale\"")]).await;
        assert_eq!(status, StatusCode::OK);

        let response = router(ASSETS)
            .oneshot(Request::head("/index.html").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "16");
        assert!(response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty());

        let response = router(ASSETS)
            .oneshot(Request::post("/index.html").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
