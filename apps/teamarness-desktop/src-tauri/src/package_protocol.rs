use tauri::http::{
    Method, Request, Response, StatusCode, Uri,
    header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_TYPE, HeaderName},
};
use teamarness_package_host::{
    PackageHostCommand, PackageHostOutcome, ReadUiResource, UiResourceToken,
};

use crate::DesktopState;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

#[derive(Debug)]
struct ResourceRequest {
    token: String,
    relative_path: String,
    head: bool,
}

pub async fn serve(state: DesktopState, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let parsed = match parse_request(request.method(), request.uri()) {
        Ok(value) => value,
        Err(status) => return response(status, "text/plain; charset=utf-8", Vec::new()),
    };
    let Some(mount) = state.mount_for_token(&parsed.token).await else {
        return response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            Vec::new(),
        );
    };
    let token = match UiResourceToken::new(parsed.token) {
        Ok(value) => value,
        Err(_) => {
            return response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Vec::new(),
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
        ),
        _ => response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            Vec::new(),
        ),
    }
}

fn parse_request(method: &Method, uri: &Uri) -> Result<ResourceRequest, StatusCode> {
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
    let decoded = String::from_utf8(output).ok()?;
    decoded.is_ascii().then_some(decoded)
}

fn response(status: StatusCode, media_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, media_type)
        .header(NOSNIFF, "nosniff")
        .header(CACHE_CONTROL, "no-store")
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
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
    fn every_response_disables_sniffing_and_storage() {
        let response = response(StatusCode::OK, "text/javascript", b"export {}".to_vec());
        assert_eq!(response.headers()[CONTENT_TYPE], "text/javascript");
        assert_eq!(response.headers()[NOSNIFF], "nosniff");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[test]
    fn csp_allows_only_the_admitted_scheme_for_package_resources() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = config["app"]["security"]["csp"].as_str().unwrap();
        assert!(csp.contains("script-src 'self' teamarness-package:"));
        assert!(csp.contains("connect-src ipc: http://ipc.localhost teamarness-package:"));
        assert!(!csp.contains("file:"));
        assert!(!csp.contains("https:"));
    }
}
