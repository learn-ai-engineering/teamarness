mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use teamarness_headless::ErrorCode;
use teamarness_headless::control::{
    self, ControlServer, Request, Response, read_frame, write_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{Notify, oneshot};
use tokio::time::{Instant, timeout};

#[tokio::test]
async fn stop_closes_admission_even_for_previously_connected_incomplete_frames() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let mut client = UnixStream::connect(fixture.socket()).await.unwrap();
    let status = app.status().await;
    let (stop, stopped) = oneshot::channel();
    let accepting = server.accept_until(
        move |_| {
            let status = status.clone();
            async { Ok(status) }
        },
        async { stopped.await.unwrap() },
    );
    tokio::pin!(accepting);
    // Poll the accept loop against the already connected peer before sending the stop event.
    tokio::select! { biased; _ = &mut accepting => panic!("stopped without signal"), _ = tokio::task::yield_now() => {} }
    stop.send(Instant::now()).unwrap();
    let mut drain = accepting.await;
    write_frame(&mut client, &support::request(7))
        .await
        .unwrap();
    // The request cannot execute after admission closes. A pre-admission close is certain.
    let result = read_frame::<Response>(&mut client).await;
    assert!(result.is_err() || result.unwrap().result.is_err());
    drain.finish().await.unwrap();
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn frame_prefix_bound_and_closed_json_shapes_are_enforced() {
    let (mut writer, mut reader) = tokio::io::duplex(128);
    writer
        .write_u32((control::MAX_FRAME_BYTES + 1) as u32)
        .await
        .unwrap();
    assert_eq!(
        read_frame::<Request>(&mut reader).await.unwrap_err().code,
        ErrorCode::FrameTooLarge
    );
    for value in [
        serde_json::json!({"major":1,"id":1,"operation":{"kind":"status","extra":true}}),
        serde_json::json!({"major":1,"id":1,"operation":{"kind":"arbitrary_host_command"}}),
        serde_json::json!({"major":1,"id":1,"operation":{"kind":"status"},"extra":true}),
        serde_json::json!({"major":1,"operation":{"kind":"status"}}),
    ] {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        write_frame(&mut writer, &value).await.unwrap();
        assert_eq!(
            read_frame::<Request>(&mut reader).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
}

#[tokio::test(start_paused = true)]
async fn slow_frame_read_and_whole_write_have_absolute_five_second_deadlines() {
    let (mut writer, mut reader) = tokio::io::duplex(8);
    writer.write_u32(100).await.unwrap();
    let read = tokio::spawn(async move { read_frame::<Request>(&mut reader).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    writer.write_all(b"x").await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(read.await.unwrap().unwrap_err().code, ErrorCode::Timeout);
    let (mut writer, mut reader) = tokio::io::duplex(8);
    let write = tokio::spawn(async move { write_frame(&mut writer, &"x".repeat(1024)).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    let mut progress = [0; 4];
    reader.read_exact(&mut progress).await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(write.await.unwrap().unwrap_err().code, ErrorCode::Timeout);
}

#[tokio::test]
async fn unsupported_version_returns_matching_id_and_same_uid_peers_can_exchange() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let status = app.status().await;
    let (stop, stopped) = oneshot::channel();
    let serving = async {
        let mut drain = server
            .accept_until(
                move |_| {
                    let status = status.clone();
                    async { Ok(status) }
                },
                async { stopped.await.unwrap() },
            )
            .await;
        drain.finish().await.unwrap();
    };
    let clients = async {
        let mut request = support::request(42);
        request.major = 2;
        let response = control::request(&fixture.config.data_directory, request)
            .await
            .unwrap();
        assert_eq!(response.id, 42);
        assert_eq!(
            response.result.unwrap_err().code,
            ErrorCode::UnsupportedVersion
        );
        let response = control::request(&fixture.config.data_directory, support::request(43))
            .await
            .unwrap();
        assert_eq!(response.id, 43);
        assert_eq!(
            response.result.unwrap().installation,
            app.status().await.installation
        );
        stop.send(Instant::now()).unwrap();
    };
    tokio::join!(serving, clients);
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn disconnected_observers_retain_eight_operation_permits_until_owned_work_finishes() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let entered = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let notification = Arc::new(Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let status = app.status().await;
    let (stop, stopped) = oneshot::channel();
    let dispatch = {
        let (entered, completed, notification, release) = (
            entered.clone(),
            completed.clone(),
            notification.clone(),
            release.clone(),
        );
        move |_| {
            let (entered, completed, notification, release, status) = (
                entered.clone(),
                completed.clone(),
                notification.clone(),
                release.clone(),
                status.clone(),
            );
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                notification.notify_one();
                release.acquire().await.unwrap().forget();
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(status)
            }
        }
    };
    let serving = async {
        let mut drain = server
            .accept_until(dispatch, async { stopped.await.unwrap() })
            .await;
        drain.finish().await.unwrap();
    };
    let clients = async {
        for id in 0..8 {
            let mut stream = UnixStream::connect(fixture.socket()).await.unwrap();
            write_frame(&mut stream, &support::request(id))
                .await
                .unwrap();
            timeout(Duration::from_secs(5), async {
                while entered.load(Ordering::SeqCst) <= id as usize {
                    notification.notified().await;
                }
            })
            .await
            .unwrap();
            drop(stream);
        }
        let busy = control::request(&fixture.config.data_directory, support::request(99))
            .await
            .unwrap();
        let error = busy.result.unwrap_err();
        assert_eq!(error.code, ErrorCode::Busy);
        assert!(!error.outcome_uncertain);
        assert_eq!(completed.load(Ordering::SeqCst), 0);
        release.add_permits(8);
        stop.send(Instant::now()).unwrap();
    };
    tokio::join!(serving, clients);
    assert_eq!(entered.load(Ordering::SeqCst), 8);
    assert_eq!(completed.load(Ordering::SeqCst), 8);
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn unsafe_occupants_are_unchanged_and_stale_owned_socket_is_replaced_only_after_lock() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let target = fixture.root.path().join("unrelated");
    std::fs::write(&target, b"untouched").unwrap();
    symlink(&target, fixture.socket()).unwrap();
    assert!(ControlServer::bind(&app).await.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
    std::fs::remove_file(fixture.socket()).unwrap();
    std::fs::write(fixture.socket(), b"not a socket").unwrap();
    assert!(ControlServer::bind(&app).await.is_err());
    assert_eq!(std::fs::read(fixture.socket()).unwrap(), b"not a socket");
    std::fs::remove_file(fixture.socket()).unwrap();
    let stale = std::os::unix::net::UnixListener::bind(fixture.socket()).unwrap();
    std::fs::set_permissions(fixture.socket(), std::fs::Permissions::from_mode(0o600)).unwrap();
    drop(stale);
    assert!(
        teamarness_headless::application::Application::open_with(
            fixture.config.clone(),
            teamarness_core::CoreRuntime::open_loopback_for_tests
        )
        .await
        .is_err()
    );
    assert!(
        fixture.socket().exists(),
        "losing opener cannot unlink even a stale endpoint"
    );
    let server = ControlServer::bind(&app).await.unwrap();
    assert!(
        ControlServer::bind(&app).await.is_err(),
        "a live listener is not stale"
    );
    drop(server);
    assert!(!fixture.socket().exists());
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn connection_cap_refuses_seventeenth_peer_without_a_wait_queue() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let status = app.status().await;
    let (stop, stopped) = oneshot::channel();
    let serving = async {
        let mut drain = server
            .accept_until(
                move |_| {
                    let status = status.clone();
                    async { Ok(status) }
                },
                async { stopped.await.unwrap() },
            )
            .await;
        drain.finish().await.unwrap();
    };
    let clients = async {
        let mut held = Vec::new();
        for _ in 0..control::MAX_CONNECTIONS {
            held.push(UnixStream::connect(fixture.socket()).await.unwrap());
        }
        let mut extra = UnixStream::connect(fixture.socket()).await.unwrap();
        let mut byte = [0];
        assert_eq!(
            timeout(Duration::from_secs(2), extra.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        drop(held);
        stop.send(Instant::now()).unwrap();
    };
    tokio::join!(serving, clients);
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn maximum_frame_round_trips_and_oversize_output_writes_no_prefix() {
    let (mut writer, mut reader) = tokio::io::duplex(4096);
    let value = "x".repeat(control::MAX_FRAME_BYTES - 2); // JSON quotes take two bytes.
    let writing = async {
        write_frame(&mut writer, &value).await.unwrap();
    };
    let reading = async {
        assert_eq!(
            read_frame::<String>(&mut reader).await.unwrap().len(),
            control::MAX_FRAME_BYTES - 2
        );
    };
    tokio::join!(writing, reading);
    let (mut writer, mut reader) = tokio::io::duplex(4096);
    assert_eq!(
        write_frame(&mut writer, &"x".repeat(control::MAX_FRAME_BYTES))
            .await
            .unwrap_err()
            .code,
        ErrorCode::FrameTooLarge
    );
    drop(writer);
    assert_eq!(
        reader.read_u8().await.unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn terminal_dispatch_failure_is_reported_and_shutdown_drain_is_uncertain() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let (stop, stopped) = oneshot::channel();
    let serving = async {
        let mut drain = server
            .accept_until(
                |_| async {
                    panic!("fixture dispatch failure");
                    #[allow(unreachable_code)]
                    Ok(std::future::pending().await)
                },
                async { stopped.await.unwrap() },
            )
            .await;
        let error = drain.finish().await.unwrap_err();
        assert!(error.outcome_uncertain);
    };
    let client = async {
        assert!(
            control::request(&fixture.config.data_directory, support::request(1))
                .await
                .is_err()
        );
        // Poll the public status until the completed task has been reaped by the accept loop.
        timeout(Duration::from_secs(5), async {
            while app.status().await.terminal_operation_failures == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            app.status().await.readiness,
            teamarness_headless::application::Readiness::Degraded
        );
        stop.send(Instant::now()).unwrap();
    };
    tokio::join!(serving, client);
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn client_rejects_wrong_version_id_and_unknown_response_fields() {
    use std::os::unix::fs::PermissionsExt;
    for response in [
        serde_json::json!({"major":2,"id":1,"result":{"Err":{"code":"busy","retryable":true,"outcomeUncertain":false}}}),
        serde_json::json!({"major":1,"id":2,"result":{"Err":{"code":"busy","retryable":true,"outcomeUncertain":false}}}),
        serde_json::json!({"major":1,"id":1,"result":{"Err":{"code":"busy","retryable":true,"outcomeUncertain":false}},"extra":true}),
    ] {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = tokio::net::UnixListener::bind(root.path().join("control.sock")).unwrap();
        std::fs::set_permissions(
            root.path().join("control.sock"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _: Request = read_frame(&mut stream).await.unwrap();
            write_frame(&mut stream, &response).await.unwrap();
        };
        let client = async {
            assert!(
                control::request(root.path(), support::request(1))
                    .await
                    .is_err()
            );
        };
        tokio::join!(server, client);
    }
}

#[tokio::test]
async fn client_observes_for_ninety_seconds_not_the_incoming_request_five_second_budget() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = tokio::net::UnixListener::bind(root.path().join("control.sock")).unwrap();
    std::fs::set_permissions(
        root.path().join("control.sock"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let path = root.path().to_owned();
    let client = tokio::spawn(async move { control::request(&path, support::request(1)).await });
    let (mut stream, _) = listener.accept().await.unwrap();
    let _: Request = read_frame(&mut stream).await.unwrap();
    // The request was transmitted; now pause only observation time.
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(89)).await;
    assert!(!client.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(client.await.unwrap().unwrap_err().code, ErrorCode::Timeout);
}
