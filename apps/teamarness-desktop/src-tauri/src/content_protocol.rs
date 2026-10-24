//! The generic `teamarness-content:` scheme (ADR 0033).
//!
//! Serves `teamarness-content://localhost/<token>/<path>` from a Host content
//! view. The Shell owns the response policy: every response, including every
//! refusal, carries the same sandboxed content policy, whatever the Package
//! or the frame that loads it asks for.

use tauri::http::{
    HeaderMap, Request, Response, StatusCode,
    header::{
        ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_RANGE,
        CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderName, RANGE,
    },
};
use teamarness_package_host::{
    ContentViewToken, PackageHostCommand, PackageHostOutcome, ReadContent,
};

use crate::DesktopState;
use crate::package_protocol::parse_resource_request;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

const REFUSAL_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// Carried on every content response. The `sandbox` directive runs the
/// document opaque-origin with scripts only, even when the embedding frame
/// sets no `sandbox` attribute. `connect-src` is where network denial will
/// be tightened later.
pub(crate) const CONTENT_POLICY: &str = "sandbox allow-scripts; default-src 'none'; script-src teamarness-content: 'unsafe-inline' 'wasm-unsafe-eval'; style-src teamarness-content: 'unsafe-inline'; img-src teamarness-content: data: blob:; font-src teamarness-content: data:; media-src teamarness-content: blob:; connect-src teamarness-content:; frame-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'";

#[derive(Debug)]
struct ContentRequest {
    token: ContentViewToken,
    path: String,
    head: bool,
}

pub async fn serve(state: DesktopState, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let parsed = match parse_content_request(&request) {
        Ok(value) => value,
        Err(status) => return refusal(status),
    };
    let Ok(host) = state.package_host() else {
        return refusal(StatusCode::NOT_FOUND);
    };
    let outcome = host
        .execute(PackageHostCommand::ReadContent(ReadContent {
            token: parsed.token,
            path: parsed.path,
        }))
        .await;
    outcome_response(outcome, request.headers(), parsed.head)
}

/// Every Host refusal, whatever its code, is a bodiless 404.
fn outcome_response<E>(
    outcome: Result<PackageHostOutcome, E>,
    headers: &HeaderMap,
    head: bool,
) -> Response<Vec<u8>> {
    match outcome {
        Ok(PackageHostOutcome::Content { bytes, media_type }) => {
            content_response(bytes, &media_type, headers, head)
        }
        _ => refusal(StatusCode::NOT_FOUND),
    }
}

fn parse_content_request(request: &Request<Vec<u8>>) -> Result<ContentRequest, StatusCode> {
    let parsed = parse_resource_request(request.method(), request.uri())?;
    let token = ContentViewToken::new(parsed.token).map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(ContentRequest {
        token,
        path: parsed.relative_path,
        head: parsed.head,
    })
}

/// One satisfiable byte range, inclusive at both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeRequest {
    Full,
    Partial { first: usize, last: usize },
    Unsatisfiable,
}

/// Reads a single `bytes=a-b`, `bytes=a-` or `bytes=-n` range. A missing,
/// malformed or multiple-range header is answered with the full content.
fn range_request(headers: &HeaderMap, length: usize) -> RangeRequest {
    let Some(value) = headers.get(RANGE).and_then(|value| value.to_str().ok()) else {
        return RangeRequest::Full;
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeRequest::Full;
    };
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((start, end)) = spec.trim().split_once('-') else {
        return RangeRequest::Full;
    };
    let parse = |value: &str| -> Option<usize> {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    };
    match (start.trim(), end.trim()) {
        ("", suffix) => match parse(suffix) {
            None => RangeRequest::Full,
            Some(0) => RangeRequest::Unsatisfiable,
            Some(_) if length == 0 => RangeRequest::Unsatisfiable,
            Some(suffix) => RangeRequest::Partial {
                first: length.saturating_sub(suffix),
                last: length - 1,
            },
        },
        (first, last) => {
            let Some(first) = parse(first) else {
                return RangeRequest::Full;
            };
            let last = if last.is_empty() {
                None
            } else {
                match parse(last) {
                    Some(value) if value >= first => Some(value),
                    _ => return RangeRequest::Full,
                }
            };
            if first >= length {
                return RangeRequest::Unsatisfiable;
            }
            RangeRequest::Partial {
                first,
                last: last.map_or(length - 1, |value| value.min(length - 1)),
            }
        }
    }
}

fn content_response(
    bytes: Vec<u8>,
    media_type: &str,
    headers: &HeaderMap,
    head: bool,
) -> Response<Vec<u8>> {
    let length = bytes.len();
    match range_request(headers, length) {
        RangeRequest::Full => builder(StatusCode::OK, media_type)
            .header(ACCEPT_RANGES, "bytes")
            .body(if head { Vec::new() } else { bytes })
            .expect("content response is valid"),
        RangeRequest::Partial { first, last } => builder(StatusCode::PARTIAL_CONTENT, media_type)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_RANGE, format!("bytes {first}-{last}/{length}"))
            .body(if head {
                Vec::new()
            } else {
                bytes[first..=last].to_vec()
            })
            .expect("content response is valid"),
        RangeRequest::Unsatisfiable => {
            builder(StatusCode::RANGE_NOT_SATISFIABLE, REFUSAL_MEDIA_TYPE)
                .header(CONTENT_RANGE, format!("bytes */{length}"))
                .body(Vec::new())
                .expect("content response is valid")
        }
    }
}

/// A refusal says nothing about why: an unknown token, an unlisted path and
/// a file that failed verification look the same.
fn refusal(status: StatusCode) -> Response<Vec<u8>> {
    builder(status, REFUSAL_MEDIA_TYPE)
        .body(Vec::new())
        .expect("static content response is valid")
}

fn builder(status: StatusCode, media_type: &str) -> tauri::http::response::Builder {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, media_type)
        .header(NOSNIFF, "nosniff")
        .header(CACHE_CONTROL, "no-store")
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_SECURITY_POLICY, CONTENT_POLICY)
}

#[cfg(test)]
mod tests {
    use tauri::http::{HeaderValue, Method};

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn request(method: Method, uri: &str) -> Request<Vec<u8>> {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Vec::new())
            .unwrap()
    }

    fn ranged(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn assert_shell_policy(response: &Response<Vec<u8>>) {
        let headers = response.headers();
        assert_eq!(headers[CONTENT_SECURITY_POLICY], CONTENT_POLICY);
        assert_eq!(headers[NOSNIFF], "nosniff");
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[test]
    fn content_policy_is_exactly_the_reviewed_sandboxed_directive_set() {
        let directives: Vec<(&str, Vec<&str>)> = CONTENT_POLICY
            .split(';')
            .map(|directive| {
                let mut parts = directive.split_whitespace();
                (parts.next().unwrap(), parts.collect())
            })
            .collect();
        let expected: Vec<(&str, Vec<&str>)> = vec![
            ("sandbox", vec!["allow-scripts"]),
            ("default-src", vec!["'none'"]),
            (
                "script-src",
                vec![
                    "teamarness-content:",
                    "'unsafe-inline'",
                    "'wasm-unsafe-eval'",
                ],
            ),
            ("style-src", vec!["teamarness-content:", "'unsafe-inline'"]),
            ("img-src", vec!["teamarness-content:", "data:", "blob:"]),
            ("font-src", vec!["teamarness-content:", "data:"]),
            ("media-src", vec!["teamarness-content:", "blob:"]),
            ("connect-src", vec!["teamarness-content:"]),
            ("frame-src", vec!["'none'"]),
            ("worker-src", vec!["'none'"]),
            ("object-src", vec!["'none'"]),
            ("form-action", vec!["'none'"]),
        ];
        assert_eq!(directives, expected);
    }

    #[test]
    fn every_content_response_carries_the_shell_policy() {
        let full = content_response(
            b"<p>hi</p>".to_vec(),
            "text/html; charset=utf-8",
            &HeaderMap::new(),
            false,
        );
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(full.headers()[ACCEPT_RANGES], "bytes");
        assert_eq!(full.body(), b"<p>hi</p>");
        assert_shell_policy(&full);
        assert_shell_policy(&content_response(
            b"abc".to_vec(),
            "text/plain; charset=utf-8",
            &ranged("bytes=0-0"),
            false,
        ));
        assert_shell_policy(&content_response(
            b"abc".to_vec(),
            "text/plain; charset=utf-8",
            &ranged("bytes=9-"),
            false,
        ));
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::BAD_REQUEST,
            StatusCode::METHOD_NOT_ALLOWED,
        ] {
            let refused = refusal(status);
            assert_eq!(refused.status(), status);
            assert!(refused.body().is_empty());
            assert_shell_policy(&refused);
        }
    }

    #[test]
    fn host_refusals_and_other_outcomes_are_bodiless_404s() {
        let refused = outcome_response::<&str>(Err("content_not_found"), &HeaderMap::new(), false);
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert!(refused.body().is_empty());
        assert_shell_policy(&refused);
        let other =
            outcome_response::<&str>(Ok(PackageHostOutcome::Shutdown), &HeaderMap::new(), false);
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
        assert!(other.body().is_empty());
        let served = outcome_response::<&str>(
            Ok(PackageHostOutcome::Content {
                bytes: b"body {}".to_vec(),
                media_type: "text/css; charset=utf-8".into(),
            }),
            &HeaderMap::new(),
            false,
        );
        assert_eq!(served.status(), StatusCode::OK);
        assert_eq!(served.headers()[CONTENT_TYPE], "text/css; charset=utf-8");
        assert_eq!(served.body(), b"body {}");
    }

    #[test]
    fn head_answers_headers_without_a_body() {
        let response = content_response(
            b"abcdef".to_vec(),
            "text/plain; charset=utf-8",
            &HeaderMap::new(),
            true,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.body().is_empty());
        assert_shell_policy(&response);
    }

    #[test]
    fn accepts_get_or_head_on_localhost_with_a_decoded_path() {
        let parsed = parse_content_request(&request(
            Method::GET,
            &format!("teamarness-content://localhost/{TOKEN}/pages/caf%C3%A9%20menu.html"),
        ))
        .unwrap();
        assert_eq!(parsed.token.as_str(), TOKEN);
        assert_eq!(parsed.path, "pages/café menu.html");
        assert!(!parsed.head);
        let head = parse_content_request(&request(
            Method::HEAD,
            &format!("teamarness-content://localhost/{TOKEN}/index.html"),
        ))
        .unwrap();
        assert!(head.head);
    }

    #[test]
    fn refuses_other_methods_queries_hosts_traversal_and_bad_tokens() {
        let index = format!("teamarness-content://localhost/{TOKEN}/index.html");
        assert_eq!(
            parse_content_request(&request(Method::POST, &index)).unwrap_err(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        for value in [
            format!("{index}?pick=1"),
            format!("teamarness-content://other/{TOKEN}/index.html"),
            format!("teamarness-content://localhost/{TOKEN}/../index.html"),
            format!("teamarness-content://localhost/{TOKEN}/pages/%2e%2e/index.html"),
            format!("teamarness-content://localhost/{TOKEN}/pages/%2findex.html"),
            format!("teamarness-content://localhost/{TOKEN}/pages/%5cindex.html"),
            format!("teamarness-content://localhost/{TOKEN}/pages//index.html"),
            format!("teamarness-content://localhost/{TOKEN}/pages/"),
            format!("teamarness-content://localhost/{TOKEN}"),
        ] {
            assert_eq!(
                parse_content_request(&request(Method::GET, &value)).unwrap_err(),
                StatusCode::BAD_REQUEST,
                "{value}"
            );
        }
        for token in [
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef",
            "zz23456789abcdef0123456789abcdef",
        ] {
            assert_eq!(
                parse_content_request(&request(
                    Method::GET,
                    &format!("teamarness-content://localhost/{token}/index.html"),
                ))
                .unwrap_err(),
                StatusCode::NOT_FOUND,
                "{token}"
            );
        }
    }

    #[test]
    fn a_single_range_returns_the_requested_bytes() {
        let bytes = b"0123456789".to_vec();
        for (header, expected, content_range) in [
            ("bytes=2-5", &b"2345"[..], "bytes 2-5/10"),
            ("bytes=7-", &b"789"[..], "bytes 7-9/10"),
            ("bytes=-3", &b"789"[..], "bytes 7-9/10"),
            ("bytes=8-99", &b"89"[..], "bytes 8-9/10"),
            ("bytes=-99", &b"0123456789"[..], "bytes 0-9/10"),
        ] {
            let response = content_response(bytes.clone(), "audio/mpeg", &ranged(header), false);
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{header}");
            assert_eq!(response.body(), expected, "{header}");
            assert_eq!(response.headers()[CONTENT_RANGE], content_range, "{header}");
            assert_eq!(response.headers()[CONTENT_TYPE], "audio/mpeg");
        }
    }

    #[test]
    fn an_unsatisfiable_range_returns_416() {
        for (header, length) in [
            ("bytes=10-", 10),
            ("bytes=10-12", 10),
            ("bytes=-0", 10),
            ("bytes=0-", 0),
        ] {
            let response =
                content_response(vec![b'x'; length], "audio/mpeg", &ranged(header), false);
            assert_eq!(
                response.status(),
                StatusCode::RANGE_NOT_SATISFIABLE,
                "{header}"
            );
            assert_eq!(
                response.headers()[CONTENT_RANGE],
                format!("bytes */{length}")
            );
            assert!(response.body().is_empty());
        }
    }

    #[test]
    fn multiple_or_malformed_ranges_return_the_full_content() {
        let bytes = b"0123456789".to_vec();
        for header in [
            "bytes=0-1,4-5",
            "bytes=5-2",
            "items=0-1",
            "bytes=a-b",
            "bytes=-",
        ] {
            let response = content_response(bytes.clone(), "video/mp4", &ranged(header), false);
            assert_eq!(response.status(), StatusCode::OK, "{header}");
            assert_eq!(response.body(), &bytes, "{header}");
            assert!(response.headers().get(CONTENT_RANGE).is_none(), "{header}");
        }
    }
}
