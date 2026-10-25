use std::sync::LazyLock;

use tauri::http::{
    Method, Request, Response, StatusCode, Uri,
    header::{
        ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
        HeaderName,
    },
};
use teamarness_package_host::{
    PackageHostCommand, PackageHostOutcome, ReadUiResource, UiResourceToken,
};

use crate::DesktopState;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

/// Shell-owned frame resources answer before the Package Host is asked.
const FRAME_PREFIX: &str = "__teamarness/";

/// The Package UI Frame document. The same bytes for every mount, with no
/// inline code, no inline style, and no Package-controlled text.
const FRAME_DOCUMENT: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Package interface</title>
<link rel="stylesheet" href="frame.css">
<script type="module" src="frame-runtime.js"></script>
</head>
<body><div id="teamarness-package-host"></div></body>
</html>
"#;

const FRAME_STYLESHEET: &str = r#"html, body { margin: 0; height: 100%; }
body { background: var(--teamarness-color-background); color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); overflow: auto; }
#teamarness-package-host { display: block; min-height: 100%; }
"#;

/// The frame runtime is produced by `npm --workspace teamarness-desktop run build:frame-runtime`.
const FRAME_RUNTIME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../frame-dist/package-frame-runtime.js"
));

/// Carried on every Package resource response, so a Package UI Frame can load
/// only Shell-owned and Package-declared resources and can reach no network.
/// `'wasm-unsafe-eval'` lets it compile WebAssembly from bytes it already holds
/// (ADR 0031); `eval` and `new Function` stay refused.
const FRAME_DOCUMENT_CSP: &str = "default-src 'none'; script-src teamarness-package: 'wasm-unsafe-eval'; style-src teamarness-package:; img-src teamarness-package: data:; font-src teamarness-package:; media-src teamarness-package:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

/// The one directive a mount's frame policy varies on.
const DENIED_FRAME_SRC: &str = "frame-src 'none'";

/// A mount whose Package declares `teamarness.content-views` may nest
/// Shell-served content frames, and nothing else (ADR 0034).
const NESTED_CONTENT_FRAME_SRC: &str = "frame-src teamarness-content:";

static NESTED_CONTENT_FRAME_DOCUMENT_CSP: LazyLock<String> =
    LazyLock::new(|| FRAME_DOCUMENT_CSP.replacen(DENIED_FRAME_SRC, NESTED_CONTENT_FRAME_SRC, 1));

/// The frame document policy for one mount: the base policy, with
/// `frame-src teamarness-content:` only when the mount reports nested content.
pub(crate) fn frame_document_policy(nested_content: bool) -> &'static str {
    if nested_content {
        NESTED_CONTENT_FRAME_DOCUMENT_CSP.as_str()
    } else {
        FRAME_DOCUMENT_CSP
    }
}

#[derive(Debug)]
pub(crate) struct ResourceRequest {
    pub(crate) token: String,
    pub(crate) relative_path: String,
    pub(crate) head: bool,
}

pub async fn serve(state: DesktopState, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let parsed = match parse_request(request.method(), request.uri()) {
        Ok(value) => value,
        Err(status) => {
            return response(
                status,
                "text/plain; charset=utf-8",
                Vec::new(),
                frame_document_policy(false),
            );
        }
    };
    let Some((mount, nested_content)) = state.mount_for_token(&parsed.token).await else {
        return response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            Vec::new(),
            frame_document_policy(false),
        );
    };
    let policy = frame_document_policy(nested_content);
    if parsed.relative_path.starts_with(FRAME_PREFIX) {
        return match reserved_frame_resource(&parsed.relative_path) {
            Some((bytes, media_type)) => response(
                StatusCode::OK,
                media_type,
                if parsed.head {
                    Vec::new()
                } else {
                    bytes.to_vec()
                },
                policy,
            ),
            None => response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Vec::new(),
                policy,
            ),
        };
    }
    let token = match UiResourceToken::new(parsed.token) {
        Ok(value) => value,
        Err(_) => {
            return response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Vec::new(),
                policy,
            );
        }
    };
    let host = match state.package_host() {
        Ok(host) => host,
        Err(_) => {
            return response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Vec::new(),
                policy,
            );
        }
    };
    let outcome = host
        .execute(PackageHostCommand::ReadUiResource(ReadUiResource {
            mount,
            token,
            relative_path: parsed.relative_path,
        }))
        .await;
    match outcome {
        Ok(PackageHostOutcome::UiResource { bytes, media_type }) => response(
            StatusCode::OK,
            &media_type,
            if parsed.head { Vec::new() } else { bytes },
            policy,
        ),
        _ => response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            Vec::new(),
            policy,
        ),
    }
}

fn reserved_frame_resource(relative_path: &str) -> Option<(&'static [u8], &'static str)> {
    match relative_path {
        "__teamarness/frame.html" => Some((FRAME_DOCUMENT.as_bytes(), "text/html; charset=utf-8")),
        "__teamarness/frame.css" => Some((FRAME_STYLESHEET.as_bytes(), "text/css; charset=utf-8")),
        "__teamarness/frame-runtime.js" => Some((FRAME_RUNTIME, "text/javascript; charset=utf-8")),
        _ => None,
    }
}

fn parse_request(method: &Method, uri: &Uri) -> Result<ResourceRequest, StatusCode> {
    let parsed = parse_resource_request(method, uri)?;
    if !parsed.token.is_ascii() || !parsed.relative_path.is_ascii() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(parsed)
}

/// The rules every Shell-owned scheme shares: GET or HEAD only, host
/// `localhost`, no query, a token segment and a normalized relative path.
/// Percent-encoded UTF-8 is decoded; an encoded separator, backslash, NUL or
/// percent sign is refused.
pub(crate) fn parse_resource_request(
    method: &Method,
    uri: &Uri,
) -> Result<ResourceRequest, StatusCode> {
    let head = match *method {
        Method::GET => false,
        Method::HEAD => true,
        _ => return Err(StatusCode::METHOD_NOT_ALLOWED),
    };
    if uri.authority().map(|value| value.as_str()) != Some("localhost") || uri.query().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = uri
        .path()
        .strip_prefix('/')
        .ok_or(StatusCode::BAD_REQUEST)?;
    let decoded = decode_path(path).ok_or(StatusCode::BAD_REQUEST)?;
    let (token, relative_path) = decoded.split_once('/').ok_or(StatusCode::BAD_REQUEST)?;
    if token.is_empty()
        || relative_path.is_empty()
        || relative_path.ends_with('/')
        || relative_path.contains("//")
        || relative_path.contains('\\')
        || relative_path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(ResourceRequest {
        token: token.to_owned(),
        relative_path: relative_path.to_owned(),
        head,
    })
}

fn decode_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let encoded = bytes.get(index + 1..index + 3)?;
            let byte = u8::from_str_radix(std::str::from_utf8(encoded).ok()?, 16).ok()?;
            if matches!(byte, b'/' | b'\\' | b'\0' | b'%') {
                return None;
            }
            output.push(byte);
            index += 3;
        } else {
            if matches!(bytes[index], b'\\' | b'\0') {
                return None;
            }
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn response(
    status: StatusCode,
    media_type: &str,
    body: Vec<u8>,
    policy: &'static str,
) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, media_type)
        .header(NOSNIFF, "nosniff")
        .header(CACHE_CONTROL, "no-store")
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_SECURITY_POLICY, policy)
        .body(body)
        .expect("static protocol response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn accepts_only_get_or_head_without_selectors() {
        let uri: Uri = format!("teamarness-package://localhost/{TOKEN}/ui/index.js")
            .parse()
            .unwrap();
        assert!(!parse_request(&Method::GET, &uri).unwrap().head);
        assert!(parse_request(&Method::HEAD, &uri).unwrap().head);
        assert_eq!(
            parse_request(&Method::POST, &uri).unwrap_err(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let queried: Uri = format!("teamarness-package://localhost/{TOKEN}/ui/index.js?pick=1")
            .parse()
            .unwrap();
        assert_eq!(
            parse_request(&Method::GET, &queried).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn rejects_traversal_encoding_and_wrong_authority() {
        for value in [
            format!("teamarness-package://localhost/{TOKEN}/../secret"),
            format!("teamarness-package://localhost/{TOKEN}/ui/%2e%2e/secret"),
            format!("teamarness-package://localhost/{TOKEN}/ui/%2fsecret"),
            format!("teamarness-package://localhost/{TOKEN}/ui/%5csecret"),
            format!("teamarness-package://localhost/{TOKEN}/ui/%252fsecret"),
            format!("teamarness-package://localhost/{TOKEN}/ui//index.js"),
            format!("teamarness-package://other/{TOKEN}/ui/index.js"),
        ] {
            let uri: Uri = value.parse().unwrap();
            assert!(parse_request(&Method::GET, &uri).is_err(), "{value}");
        }
    }

    #[test]
    fn every_response_carries_the_frame_document_policy() {
        let response = response(
            StatusCode::OK,
            "text/javascript",
            b"export {}".to_vec(),
            frame_document_policy(false),
        );
        assert_eq!(response.headers()[CONTENT_TYPE], "text/javascript");
        assert_eq!(response.headers()[NOSNIFF], "nosniff");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            response.headers()[CONTENT_SECURITY_POLICY],
            FRAME_DOCUMENT_CSP
        );
    }

    #[test]
    fn a_mount_without_nested_content_denies_every_nested_frame() {
        assert_eq!(frame_document_policy(false), FRAME_DOCUMENT_CSP);
        assert!(frame_document_policy(false).contains("frame-src 'none'"));
        assert!(!frame_document_policy(false).contains("teamarness-content:"));
    }

    #[test]
    fn a_mount_with_nested_content_may_frame_only_content_views() {
        let directives = |policy: &'static str| -> Vec<(&'static str, Vec<&'static str>)> {
            policy
                .split(';')
                .map(|directive| {
                    let mut parts = directive.split_whitespace();
                    (parts.next().unwrap(), parts.collect())
                })
                .collect()
        };
        let base = directives(frame_document_policy(false));
        let nested = directives(frame_document_policy(true));
        assert_eq!(base.len(), nested.len());
        for ((base_name, base_sources), (name, sources)) in base.iter().zip(&nested) {
            assert_eq!(base_name, name);
            if *name == "frame-src" {
                assert_eq!(base_sources, &vec!["'none'"]);
                assert_eq!(sources, &vec!["teamarness-content:"]);
            } else {
                assert_eq!(base_sources, sources, "{name}");
            }
        }
        let response = response(
            StatusCode::OK,
            "text/html; charset=utf-8",
            Vec::new(),
            frame_document_policy(true),
        );
        assert_eq!(
            response.headers()[CONTENT_SECURITY_POLICY],
            frame_document_policy(true)
        );
    }

    #[test]
    fn package_resources_stay_ascii_while_shared_rules_decode_utf8() {
        let uri: Uri = format!("teamarness-package://localhost/{TOKEN}/ui/caf%C3%A9.js")
            .parse()
            .unwrap();
        assert_eq!(
            parse_request(&Method::GET, &uri).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            parse_resource_request(&Method::GET, &uri)
                .unwrap()
                .relative_path,
            "ui/café.js"
        );
    }

    #[test]
    fn serves_shell_owned_frame_resources_only_under_the_reserved_prefix() {
        assert_eq!(
            reserved_frame_resource("__teamarness/frame.html").map(|(_, media)| media),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            reserved_frame_resource("__teamarness/frame.css").map(|(_, media)| media),
            Some("text/css; charset=utf-8")
        );
        assert_eq!(
            reserved_frame_resource("__teamarness/frame-runtime.js").map(|(_, media)| media),
            Some("text/javascript; charset=utf-8")
        );
        assert!(reserved_frame_resource("__teamarness/other.js").is_none());
        assert!(reserved_frame_resource("ui/__teamarness/frame.html").is_none());
        for value in [
            format!("teamarness-package://localhost/{TOKEN}/__teamarness/"),
            format!("teamarness-package://localhost/{TOKEN}/__teamarness/../ui/index.js"),
        ] {
            let uri: Uri = value.parse().unwrap();
            assert!(parse_request(&Method::GET, &uri).is_err(), "{value}");
        }
        assert!(!"ui/__teamarness/frame.html".starts_with(FRAME_PREFIX));
        assert!("__teamarness/frame.html".starts_with(FRAME_PREFIX));
    }

    #[test]
    fn frame_document_loads_only_relative_shell_owned_resources() {
        assert!(FRAME_DOCUMENT.contains(r#"src="frame-runtime.js""#));
        assert!(FRAME_DOCUMENT.contains(r#"href="frame.css""#));
        assert!(FRAME_DOCUMENT.contains(r#"id="teamarness-package-host""#));
        assert!(!FRAME_DOCUMENT.contains("<style"));
        assert!(!FRAME_DOCUMENT.contains("style="));
        assert!(!FRAME_DOCUMENT.contains("<script>"));
        assert_eq!(FRAME_DOCUMENT.matches("<script").count(), 1);
    }

    #[test]
    fn frame_document_policy_denies_network_frames_forms_and_inline_code() {
        for directive in [
            "connect-src 'none'",
            "frame-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
            "object-src 'none'",
            "worker-src 'none'",
        ] {
            assert!(FRAME_DOCUMENT_CSP.contains(directive), "{directive}");
        }
        for forbidden in [
            "'unsafe-inline'",
            "'unsafe-eval'",
            "https:",
            "http:",
            "ipc:",
        ] {
            assert!(!FRAME_DOCUMENT_CSP.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn frame_document_policy_is_exactly_the_reviewed_directive_set() {
        let directives: Vec<(&str, Vec<&str>)> = FRAME_DOCUMENT_CSP
            .split(';')
            .map(|directive| {
                let mut parts = directive.split_whitespace();
                let name = parts.next().expect("a directive has a name");
                (name, parts.collect())
            })
            .collect();
        let expected: Vec<(&str, Vec<&str>)> = vec![
            ("default-src", vec!["'none'"]),
            (
                "script-src",
                vec!["teamarness-package:", "'wasm-unsafe-eval'"],
            ),
            ("style-src", vec!["teamarness-package:"]),
            ("img-src", vec!["teamarness-package:", "data:"]),
            ("font-src", vec!["teamarness-package:"]),
            ("media-src", vec!["teamarness-package:"]),
            ("connect-src", vec!["'none'"]),
            ("frame-src", vec!["'none'"]),
            ("worker-src", vec!["'none'"]),
            ("object-src", vec!["'none'"]),
            ("base-uri", vec!["'none'"]),
            ("form-action", vec!["'none'"]),
        ];
        assert_eq!(directives, expected);
    }

    #[test]
    fn embedded_frame_runtime_has_no_tauri_surface() {
        assert!(!FRAME_RUNTIME.is_empty());
        let runtime = std::str::from_utf8(FRAME_RUNTIME).expect("the frame runtime is UTF-8");
        for forbidden in ["__TAURI", "@tauri-apps", "ipc://", "plugin:", "invoke"] {
            assert!(
                !runtime.contains(forbidden),
                "the embedded frame runtime contains {forbidden}"
            );
        }
    }

    #[test]
    fn shell_csp_frames_package_documents_without_loading_package_code() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = config["app"]["security"]["csp"].as_str().unwrap();
        let directives: Vec<Vec<&str>> = csp
            .split(';')
            .map(|directive| directive.split_whitespace().collect())
            .collect();
        let sources = |name: &str| -> Vec<&str> {
            directives
                .iter()
                .find(|parts| parts.first() == Some(&name))
                .map(|parts| parts[1..].to_vec())
                .unwrap_or_default()
        };
        assert_eq!(sources("script-src"), vec!["'self'"]);
        assert_eq!(sources("frame-src"), vec!["teamarness-package:"]);
        assert_eq!(sources("connect-src"), vec!["ipc:", "http://ipc.localhost"]);
        for parts in &directives {
            if parts.first() == Some(&"frame-src") {
                continue;
            }
            assert!(
                !parts.contains(&"teamarness-package:"),
                "the Shell realm must not load {parts:?} from Packages"
            );
        }
        for forbidden in ["file:", "https:", "'unsafe-inline'", "'unsafe-eval'"] {
            assert!(!csp.contains(forbidden), "{forbidden}");
        }
    }
}
