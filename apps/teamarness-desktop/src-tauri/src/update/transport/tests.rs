use super::super::SystemClock;
use super::*;
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct FixtureBody {
    chunks: VecDeque<Vec<u8>>,
    pending: bool,
    delay: Duration,
}
impl Body for FixtureBody {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(async move {
            if self.pending {
                return std::future::pending().await;
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(self.chunks.pop_front())
        })
    }
}
struct FixtureHttp {
    responses: Mutex<VecDeque<Response>>,
    calls: AtomicUsize,
    pending: bool,
}
impl Http for FixtureHttp {
    fn get(&self, _url: Url) -> BoxFuture<'_, Result<Response, ErrorCategory>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.pending {
                return std::future::pending().await;
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(ErrorCategory::Network)
        })
    }
}
fn response(status: u16, location: Option<&str>, chunks: Vec<Vec<u8>>) -> Response {
    Response {
        status,
        location: location.map(str::to_owned),
        encoding: None,
        body: Box::new(FixtureBody {
            chunks: chunks.into(),
            pending: false,
            delay: Duration::ZERO,
        }),
    }
}
fn transfer(responses: Vec<Response>) -> BoundedTransfer {
    BoundedTransfer {
        http: Box::new(FixtureHttp {
            responses: Mutex::new(responses.into()),
            calls: AtomicUsize::new(0),
            pending: false,
        }),
        clock: Arc::new(SystemClock),
    }
}
fn url() -> Url {
    Url::parse(super::super::config::ENDPOINT).unwrap()
}
async fn fetch(
    transfer: &BoundedTransfer,
    resource: Resource,
) -> Result<Option<Vec<u8>>, ErrorCategory> {
    transfer
        .fetch(url(), resource, Cancellation::new(), Arc::new(|_| {}))
        .await
}

#[tokio::test]
async fn update_transport_caps_streamed_chunks_independent_of_content_length() {
    for (resource, limit) in [
        (Resource::Feed, super::super::config::FEED_LIMIT),
        (Resource::Archive, super::super::config::ARCHIVE_LIMIT),
    ] {
        // Exercise the exact production accumulator at a synthetic boundary. No
        // 512MiB fixture allocation is needed to qualify limit arithmetic.
        let mut bytes = vec![0; 3];
        append(&mut bytes, b"12345", 8).unwrap();
        assert_eq!(bytes.len(), 8);
        assert_eq!(append(&mut bytes, b"x", 8), Err(ErrorCategory::TooLarge));
        assert_eq!(resource.limit(), limit);
        let bounded = transfer(vec![response(200, None, vec![vec![0; 7], vec![1]])]);
        assert_eq!(
            bounded
                .acquire_with_limit(url(), resource, Cancellation::new(), Arc::new(|_| {}), 8)
                .await
                .unwrap()
                .unwrap()
                .len(),
            8
        );
        let oversized = transfer(vec![response(200, None, vec![vec![0; 8], vec![1]])]);
        assert_eq!(
            oversized
                .acquire_with_limit(url(), resource, Cancellation::new(), Arc::new(|_| {}), 8)
                .await,
            Err(ErrorCategory::TooLarge)
        );
    }
    let limit = super::super::config::FEED_LIMIT;
    assert_eq!(
        fetch(
            &transfer(vec![response(200, None, vec![vec![0; limit - 1], vec![1]])]),
            Resource::Feed
        )
        .await
        .unwrap()
        .unwrap()
        .len(),
        limit
    );
    assert_eq!(
        fetch(
            &transfer(vec![response(200, None, vec![vec![0; limit], vec![1]])]),
            Resource::Feed
        )
        .await,
        Err(ErrorCategory::TooLarge)
    );
}
#[tokio::test]
async fn update_transport_validates_every_redirect_and_never_follows_six_or_loops() {
    for location in [
        "http://github.com/x",
        "https://user:secret@github.com/x",
        "https://github.com.evil.test/x",
        "https://github.com:444/x",
        "https://objects.githubusercontent.com/x#fragment",
    ] {
        assert_eq!(
            fetch(
                &transfer(vec![response(302, Some(location), vec![])]),
                Resource::Feed
            )
            .await,
            Err(ErrorCategory::Metadata)
        );
    }
    assert_eq!(
        fetch(
            &transfer(vec![response(302, Some(url().as_str()), vec![])]),
            Resource::Feed
        )
        .await,
        Err(ErrorCategory::Metadata)
    );
    for count in [5, 6] {
        let mut responses: Vec<_> = (0..count)
            .map(|index| {
                response(
                    302,
                    Some(&format!(
                        "https://release-assets.githubusercontent.com/{index}"
                    )),
                    vec![],
                )
            })
            .collect();
        responses.push(response(200, None, vec![b"feed".to_vec()]));
        let result = fetch(&transfer(responses), Resource::Feed).await;
        if count == 5 {
            assert_eq!(result.unwrap().unwrap(), b"feed");
        } else {
            assert_eq!(result, Err(ErrorCategory::Metadata));
        }
    }
    for host in HOSTS {
        assert!(validate_url(&Url::parse(&format!("https://{host}/asset")).unwrap()).is_ok());
    }
}
#[tokio::test]
async fn update_transport_404_first_release_errors_and_encoded_bodies() {
    assert_eq!(
        fetch(&transfer(vec![response(404, None, vec![])]), Resource::Feed)
            .await
            .unwrap(),
        None
    );
    for status in [400, 401, 403, 500, 503, 204] {
        assert_eq!(
            fetch(
                &transfer(vec![response(status, None, vec![])]),
                Resource::Feed
            )
            .await,
            Err(ErrorCategory::Network)
        );
    }
    assert_eq!(
        fetch(
            &transfer(vec![response(404, None, vec![])]),
            Resource::Archive
        )
        .await,
        Err(ErrorCategory::Network)
    );
    let mut encoded = response(200, None, vec![b"compressed".to_vec()]);
    encoded.encoding = Some("gzip".into());
    assert_eq!(
        fetch(&transfer(vec![encoded]), Resource::Feed).await,
        Err(ErrorCategory::Metadata)
    );
}
#[tokio::test(start_paused = true)]
async fn update_transport_connection_inactivity_total_deadlines_and_cancellation() {
    let connecting = BoundedTransfer {
        http: Box::new(FixtureHttp {
            responses: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
            pending: true,
        }),
        clock: Arc::new(SystemClock),
    };
    let start = Instant::now();
    assert_eq!(
        fetch(&connecting, Resource::Feed).await,
        Err(ErrorCategory::Timeout)
    );
    assert_eq!(Instant::now() - start, Duration::from_secs(10));
    for resource in [Resource::Feed, Resource::Archive] {
        let mut empty = response(200, None, vec![]);
        empty.body = Box::new(FixtureBody {
            chunks: vec![vec![]; 100].into(),
            pending: false,
            delay: Duration::from_secs(20),
        });
        let start = Instant::now();
        assert_eq!(
            fetch(&transfer(vec![empty]), resource).await,
            Err(ErrorCategory::Timeout)
        );
        assert_eq!(Instant::now() - start, Duration::from_secs(30));
        let mut stalled = response(200, None, vec![]);
        stalled.body = Box::new(FixtureBody {
            chunks: VecDeque::new(),
            pending: true,
            delay: Duration::ZERO,
        });
        let start = Instant::now();
        assert_eq!(
            fetch(&transfer(vec![stalled]), resource).await,
            Err(ErrorCategory::Timeout)
        );
        assert_eq!(Instant::now() - start, Duration::from_secs(30));
        // Chunks remain active, but total deadline is not extended by progress.
        let mut active = response(200, None, vec![]);
        active.body = Box::new(FixtureBody {
            chunks: vec![vec![0]; 100].into(),
            pending: false,
            delay: Duration::from_secs(20),
        });
        let start = Instant::now();
        assert_eq!(
            fetch(&transfer(vec![active]), resource).await,
            Err(ErrorCategory::Timeout)
        );
        assert_eq!(Instant::now() - start, resource.total());
    }
    let cancel = Cancellation::new();
    let future = connecting.fetch(url(), Resource::Archive, cancel.clone(), Arc::new(|_| {}));
    tokio::pin!(future);
    tokio::select! { biased; _ = &mut future => panic!("must still connect"), _ = tokio::task::yield_now() => () }
    cancel.cancel();
    assert_eq!(future.await, Err(ErrorCategory::Cancelled));
}

#[tokio::test]
async fn update_reqwest_production_client_rejects_http_without_network() {
    let adapter = ReqwestHttp(client().unwrap());
    assert!(
        adapter
            .get(Url::parse("http://127.0.0.1:1/not-a-production-bypass").unwrap())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn update_real_reqwest_wiring_no_redirect_decompression_credentials_or_content_length_trust()
{
    use std::io::{Read, Write};
    // Only this private test adapter can disable HTTPS. Production constructs
    // client() and validates HTTPS before every hop; there is no runtime switch.
    for (headers, body, expected_status, expected_bytes) in [
        (
            "Location: http://127.0.0.1:1/must-not-follow\r\n",
            b"abc".as_slice(),
            302,
            Some(b"abc".as_slice()),
        ),
        (
            "Content-Encoding: gzip\r\n",
            b"not-decompressed".as_slice(),
            200,
            Some(b"not-decompressed".as_slice()),
        ),
        (
            "",
            b"no-content-length".as_slice(),
            200,
            Some(b"no-content-length".as_slice()),
        ),
        (
            "Content-Length: 1\r\n",
            b"false-length".as_slice(),
            200,
            Some(b"f".as_slice()),
        ),
        (
            "Content-Length: 999\r\n",
            b"truncated".as_slice(),
            200,
            None,
        ),
    ] {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut part = [0; 1024];
                let count = connection.read(&mut part).unwrap();
                bytes.extend_from_slice(&part[..count]);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8(bytes).unwrap().to_lowercase();
            for forbidden in [
                "authorization:",
                "proxy-authorization:",
                "cookie:",
                "referer:",
            ] {
                assert!(!request.contains(forbidden));
            }
            assert!(request.contains("accept-encoding: identity"));
            write!(
                connection,
                "HTTP/1.1 {expected_status} Test\r\n{headers}Connection: close\r\n\r\n"
            )
            .unwrap();
            connection.write_all(body).unwrap();
        });
        let adapter = ReqwestHttp(client_builder().https_only(false).build().unwrap());
        let mut response = adapter
            .get(Url::parse(&format!("http://{address}/asset")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status, expected_status);
        let mut bytes = Vec::new();
        let mut error = None;
        loop {
            match response.body.chunk().await {
                Ok(Some(chunk)) => bytes.extend(chunk),
                Ok(None) => break,
                Err(category) => {
                    error = Some(category);
                    break;
                }
            }
        }
        match expected_bytes {
            Some(expected) => {
                assert_eq!(bytes, expected);
                assert!(error.is_none());
            }
            None => assert_eq!(error, Some(ErrorCategory::Network)),
        }
        // A false short length yields truncated bytes, which still must pass the
        // official verifier; Content-Length is never a Ready signal or size authority.
        if headers.contains("gzip") {
            assert_eq!(response.encoding.as_deref(), Some("gzip"));
        }
        server.join().unwrap();
    }
}
