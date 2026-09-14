mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use teamarness_package_host::{
    CloseUiMount, OpenUiMount, PackageErrorCode, PackageHost, PackageHostCommand,
    PackageHostOutcome, ReadUiResource, UiResourceToken,
};

#[tokio::test]
async fn opaque_mount_token_resolves_only_admitted_resources() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let instance = host.snapshot().await.instances[0].key.clone();
    let (mount, loader) = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader),
        other => panic!("unexpected outcome: {other:?}"),
    };
    assert_eq!(loader.token.as_str().len(), 32);
    assert_eq!(loader.entrypoint, "ui/index.js");
    assert_eq!(loader.ui_host_version, 1);
    assert_eq!(loader.package.identity.as_str(), "dev.teamarness.fixture");
    assert!(!loader.contract_digest.is_empty());

    let read = |path: &str, token: UiResourceToken| {
        host.execute(PackageHostCommand::ReadUiResource(ReadUiResource {
            mount: mount.clone(),
            token,
            relative_path: path.to_owned(),
        }))
    };
    match read(&loader.entrypoint, loader.token.clone())
        .await
        .unwrap()
    {
        PackageHostOutcome::UiResource { bytes, media_type } => {
            assert!(
                String::from_utf8(bytes)
                    .unwrap()
                    .starts_with("export function mount")
            );
            assert_eq!(media_type, "text/javascript");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    match read("ui/icon.svg", loader.token.clone()).await.unwrap() {
        PackageHostOutcome::UiResource { bytes, media_type } => {
            assert_eq!(bytes, b"<svg/>");
            assert_eq!(media_type, "image/svg+xml");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let wrong = UiResourceToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(
        read("ui/index.js", wrong).await.unwrap_err().code,
        PackageErrorCode::StaleMount
    );
    for path in [
        "",
        "/ui/index.js",
        "ui//index.js",
        "ui/./index.js",
        "ui/../index.js",
        "ui\\index.js",
        "ui/%2findex.js",
        "ui/%252findex.js",
        "ui/index.js?x=1",
        "ui/contract.json",
        "ui/missing.js",
    ] {
        assert_eq!(
            read(path, loader.token.clone()).await.unwrap_err().code,
            PackageErrorCode::UiLoadFailed,
            "{path}"
        );
    }

    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount {
        mount: mount.clone(),
    }))
    .await
    .unwrap();
    assert_eq!(
        read("ui/index.js", loader.token).await.unwrap_err().code,
        PackageErrorCode::StaleMount
    );
}

#[tokio::test]
async fn resource_recheck_rejects_tampered_bytes_and_mode() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let instance = host.snapshot().await.instances[0].key.clone();
    let (mount, loader) = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader),
        _ => unreachable!(),
    };
    let cache = fs::read_dir(fixture._private.join("packages/ui-cache/v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entrypoint = cache.join("ui/index.js");
    let admitted_size = fs::metadata(&entrypoint).unwrap().len() as usize;
    fs::set_permissions(&entrypoint, fs::Permissions::from_mode(0o600)).unwrap();
    let command = || {
        PackageHostCommand::ReadUiResource(ReadUiResource {
            mount: mount.clone(),
            token: loader.token.clone(),
            relative_path: loader.entrypoint.clone(),
        })
    };
    assert_eq!(
        host.execute(command()).await.unwrap_err().code,
        PackageErrorCode::UiLoadFailed
    );
    fs::write(&entrypoint, vec![b'x'; admitted_size]).unwrap();
    fs::set_permissions(&entrypoint, fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(
        host.execute(command()).await.unwrap_err().code,
        PackageErrorCode::UiLoadFailed
    );
    fs::set_permissions(&entrypoint, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&entrypoint, b"short").unwrap();
    fs::set_permissions(&entrypoint, fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(
        host.execute(command()).await.unwrap_err().code,
        PackageErrorCode::UiLoadFailed
    );
}
