//! `teamarness.content-views` 1.0 through a real Host and a real Package process: a Package
//! opens views over files in its own Data Directory, and `ReadContent` serves exactly the
//! listed, verified files with allowlisted media types.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use serde_json::{Value, json};
use teamarness_package_host::{
    ContentViewToken, DisposePackage, OpenUiMount, PackageErrorCode, PackageHost,
    PackageHostCommand, PackageHostOutcome, PackageValue, ReadContent, UiMountId, UiRequest,
    UiRequestId,
};

const FAMILY: &str = "teamarness.content-views";
const PACKAGE: &str = "dev.teamarness.fixture";

struct Opened {
    host: PackageHost,
    mount: UiMountId,
    data: PathBuf,
    fixture: common::Fixture,
    next: std::cell::Cell<u64>,
}

impl Opened {
    async fn new() -> Self {
        let fixture = common::Fixture::node_with_capabilities(
            &[("fixture", PACKAGE)],
            &[json!({"identity":FAMILY,"major":1,"minMinor":0,"maxMinor":0})],
        )
        .await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let mount = open_mount(&host).await.0;
        let data = fixture
            ._private
            .join("packages/data/v1")
            .join(fixture.core.public_identity().to_string())
            .join(fixture.core.list_workspaces().unwrap()[0].id().to_string())
            .join(common::digest(PACKAGE.as_bytes()));
        assert!(data.is_dir(), "the Package Data Directory exists");
        Self {
            host,
            mount,
            data,
            fixture,
            next: std::cell::Cell::new(0),
        }
    }

    /// Writes one file into the Package Data Directory and returns its listing entry.
    fn write(&self, path: &str, source: &str, bytes: &[u8]) -> Value {
        let target = self.data.join(source);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, bytes).unwrap();
        json!({
            "path": path,
            "sha256": common::digest(bytes),
            "size": bytes.len(),
            "source": {"kind":"file","path":source},
        })
    }

    async fn call(&self, method: &str, arguments: Value) -> BTreeMap<String, PackageValue> {
        let sequence = self.next.get();
        self.next.set(sequence + 1);
        let payload = serde_json::from_value::<PackageValue>(json!({
            "capability": FAMILY,
            "method": method,
            "arguments": arguments,
        }))
        .unwrap();
        let outcome = self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.mount.clone(),
                request: UiRequestId::new(format!("content-{sequence}")).unwrap(),
                operation: "fixture.capability".into(),
                payload,
            }))
            .await
            .unwrap();
        let PackageHostOutcome::UiResponse {
            payload: PackageValue::Object(mut response),
        } = outcome
        else {
            panic!("unexpected capability response")
        };
        match response.remove("result") {
            Some(PackageValue::Object(result)) => result,
            Some(_) => panic!("capability result was not an object"),
            None => response,
        }
    }

    async fn open(
        &self,
        entry: &str,
        files: Vec<Value>,
    ) -> Result<(ContentViewToken, String), String> {
        let result = self
            .call("open", json!({"entry":entry,"files":files}))
            .await;
        if let Some(PackageValue::String(error)) = result.get("error") {
            return Err(error.clone());
        }
        let (Some(PackageValue::String(view)), Some(PackageValue::String(url))) =
            (result.get("view"), result.get("url"))
        else {
            panic!("open answered {result:?}")
        };
        Ok((ContentViewToken::new(view.clone()).unwrap(), url.clone()))
    }

    async fn read(
        &self,
        token: &ContentViewToken,
        path: &str,
    ) -> Result<(Vec<u8>, String), PackageErrorCode> {
        match self
            .host
            .execute(PackageHostCommand::ReadContent(ReadContent {
                token: token.clone(),
                path: path.into(),
            }))
            .await
        {
            Ok(PackageHostOutcome::Content { bytes, media_type }) => Ok((bytes, media_type)),
            Ok(other) => panic!("unexpected read outcome {other:?}"),
            Err(error) => Err(error.code),
        }
    }
}

async fn open_mount(host: &PackageHost) -> (UiMountId, bool) {
    let instance = host.snapshot().await.instances[0].key.clone();
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader.nested_content),
        other => panic!("unexpected mount outcome: {other:?}"),
    }
}

/// A small listing: one entry page that every test can open.
fn single(opened: &Opened) -> Vec<Value> {
    vec![opened.write("index.html", "content/index", b"<h1>Exhibit</h1>")]
}

#[tokio::test]
async fn open_then_read_every_listed_file_returns_its_bytes_and_media_type() {
    let opened = Opened::new().await;
    let fixtures: [(&str, &str, &[u8], &str); 6] = [
        (
            "index.html",
            "store/a",
            b"<script type=module src=app.mjs></script>",
            "text/html; charset=utf-8",
        ),
        (
            "pages/page 2.html",
            "store/b",
            b"<a href=../index.html>back</a>",
            "text/html; charset=utf-8",
        ),
        (
            "app.mjs",
            "store/c",
            b"export const ok = true;",
            "text/javascript; charset=utf-8",
        ),
        (
            "assets/dot.png",
            "store/d",
            b"\x89PNG\r\n\x1a\n",
            "image/png",
        ),
        ("data.json", "store/e", b"{\"ok\":true}", "application/json"),
        (
            "mod.wasm",
            "store/nested/f",
            b"\0asm\x01\0\0\0",
            "application/wasm",
        ),
    ];
    let files = fixtures
        .iter()
        .map(|(path, source, bytes, _)| opened.write(path, source, bytes))
        .collect();
    let (view, url) = opened.open("index.html", files).await.unwrap();
    assert_eq!(
        url,
        format!(
            "teamarness-content://localhost/{}/index.html",
            view.as_str()
        )
    );
    for (path, _, bytes, media_type) in fixtures {
        // Twice: the first read verifies, the second is served from the cached verdict.
        for _ in 0..2 {
            let (served, served_type) = opened.read(&view, path).await.unwrap();
            assert_eq!(served, bytes, "{path}");
            assert_eq!(served_type, media_type, "{path}");
        }
    }
    let (_, spaced) = opened
        .open(
            "pages/page 2.html",
            vec![opened.write(
                "pages/page 2.html",
                "store/b",
                b"<a href=../index.html>back</a>",
            )],
        )
        .await
        .unwrap();
    assert!(spaced.ends_with("/pages/page%202.html"), "{spaced}");
}

#[tokio::test]
async fn an_unknown_token_or_an_unlisted_path_is_not_found() {
    let opened = Opened::new().await;
    opened.write("secret.html", "content/secret", b"not listed");
    let (view, _) = opened.open("index.html", single(&opened)).await.unwrap();
    let unknown = ContentViewToken::new("0123456789abcdef0123456789abcdef").unwrap();
    assert_eq!(
        opened.read(&unknown, "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
    for path in [
        "secret.html",
        "content/index",
        "../index.html",
        "INDEX.html",
        "",
    ] {
        assert_eq!(
            opened.read(&view, path).await.unwrap_err(),
            PackageErrorCode::ContentNotFound,
            "{path:?}"
        );
    }
    assert!(opened.read(&view, "index.html").await.is_ok());
}

#[tokio::test]
async fn a_file_modified_after_open_is_refused() {
    let opened = Opened::new().await;
    let files = vec![
        opened.write("index.html", "content/index", b"original"),
        opened.write("later.html", "content/later", b"original"),
    ];
    let (view, _) = opened.open("index.html", files).await.unwrap();
    // Same size, different bytes, before the first read: the hash check refuses it.
    fs::write(opened.data.join("content/index"), b"replaced").unwrap();
    assert_eq!(
        opened.read(&view, "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
    // The refusal is cached: restoring the bytes does not revive the file in this view.
    fs::write(opened.data.join("content/index"), b"original").unwrap();
    assert_eq!(
        opened.read(&view, "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
    // A verified file whose size changes afterwards is refused too.
    assert!(opened.read(&view, "later.html").await.is_ok());
    fs::write(opened.data.join("content/later"), b"longer than before").unwrap();
    assert_eq!(
        opened.read(&view, "later.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
    // A new view over the restored bytes serves them.
    let (fresh, _) = opened
        .open(
            "index.html",
            vec![opened.write("index.html", "content/index", b"original")],
        )
        .await
        .unwrap();
    assert_eq!(
        opened.read(&fresh, "index.html").await.unwrap().0,
        b"original"
    );
}

#[tokio::test]
async fn symlinks_non_regular_files_and_escapes_are_refused_at_open() {
    let opened = Opened::new().await;
    let outside = opened.fixture._root.path().join("outside.html");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, opened.data.join("link")).unwrap();
    fs::create_dir_all(opened.data.join("real")).unwrap();
    fs::write(opened.data.join("real/page"), b"outside").unwrap();
    symlink(opened.data.join("real"), opened.data.join("linked-dir")).unwrap();
    fs::create_dir_all(opened.data.join("directory")).unwrap();
    let entry = |source: &str| {
        json!({
            "path": "index.html",
            "sha256": common::digest(b"outside"),
            "size": 7,
            "source": {"kind":"file","path":source},
        })
    };
    for source in [
        "link",
        "linked-dir/page",
        "directory",
        "missing",
        "../outside.html",
        "/etc/hosts",
        "real/./page",
    ] {
        assert_eq!(
            opened
                .open("index.html", vec![entry(source)])
                .await
                .unwrap_err(),
            "invalid_content_path",
            "{source}"
        );
    }
    assert!(
        opened
            .open("index.html", vec![entry("real/page")])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn an_extension_outside_the_allowlist_is_an_unsupported_media_type() {
    let opened = Opened::new().await;
    for path in [
        "index.exe",
        "image.PNG",
        "Index.HTML",
        "README",
        "archive.tar.gz",
    ] {
        let files = vec![opened.write(path, "content/file", b"bytes")];
        assert_eq!(
            opened.open(path, files).await.unwrap_err(),
            "unsupported_media_type",
            "{path}"
        );
    }
}

#[tokio::test]
async fn bounds_on_files_totals_sizes_and_paths_are_refused() {
    let opened = Opened::new().await;
    let declared = |path: String, size: u64| {
        json!({
            "path": path,
            "sha256": "0".repeat(64),
            "size": size,
            "source": {"kind":"file","path":"content/none"},
        })
    };
    // More than 1,000 files.
    let many = (0..1001)
        .map(|index| declared(format!("f{index}.txt"), 1))
        .collect::<Vec<_>>();
    assert_eq!(
        opened.open("f0.txt", many).await.unwrap_err(),
        "content_too_large"
    );
    // More than 64 MiB declared in total, every file within its own bound.
    let total = (0..5)
        .map(|index| declared(format!("f{index}.mp4"), 16 * 1024 * 1024))
        .collect::<Vec<_>>();
    assert_eq!(
        opened.open("f0.mp4", total).await.unwrap_err(),
        "content_too_large"
    );
    // One file over 16 MiB.
    assert_eq!(
        opened
            .open(
                "big.mp4",
                vec![declared("big.mp4".into(), 16 * 1024 * 1024 + 1)]
            )
            .await
            .unwrap_err(),
        "content_too_large"
    );
    // A path over 128 bytes.
    let long = format!("{}.html", "p".repeat(124));
    assert_eq!(long.len(), 129);
    assert_eq!(
        opened
            .open(&long, vec![declared(long.clone(), 1)])
            .await
            .unwrap_err(),
        "invalid_content_path"
    );
    // The bounds themselves are accepted.
    let longest = format!("{}.html", "p".repeat(123));
    let files = vec![opened.write(&longest, "content/longest", b"ok")];
    assert!(opened.open(&longest, files).await.is_ok());
}

#[tokio::test]
async fn opening_the_33rd_view_closes_the_least_recently_opened() {
    let opened = Opened::new().await;
    let mut views = Vec::new();
    for _ in 0..33 {
        views.push(opened.open("index.html", single(&opened)).await.unwrap().0);
    }
    assert_eq!(
        opened.read(&views[0], "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound,
        "the first view was evicted"
    );
    for view in &views[1..] {
        assert!(opened.read(view, "index.html").await.is_ok());
    }
    // `close` ends one view and is idempotent.
    let closed = opened
        .call("close", json!({"view": views[1].as_str()}))
        .await;
    assert_eq!(closed.get("closed"), Some(&PackageValue::Boolean(true)));
    let again = opened
        .call("close", json!({"view": views[1].as_str()}))
        .await;
    assert_eq!(again.get("closed"), Some(&PackageValue::Boolean(false)));
    assert_eq!(
        opened.read(&views[1], "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
}

#[tokio::test]
async fn generation_end_closes_every_view_of_the_instance() {
    let opened = Opened::new().await;
    let (first, _) = opened.open("index.html", single(&opened)).await.unwrap();
    let (second, _) = opened.open("index.html", single(&opened)).await.unwrap();
    assert!(opened.read(&first, "index.html").await.is_ok());

    // The Backend crashes; the Host replaces its generation.
    let abort = opened
        .host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: opened.mount.clone(),
            request: UiRequestId::new("abort-generation").unwrap(),
            operation: "fixture.abort".into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await;
    assert!(abort.is_err());
    common::eventually(
        std::time::Duration::from_secs(30),
        "the crashed generation is replaced",
        || async { opened.host.snapshot().await.instances[0].process_generation == Some(2) },
    )
    .await;
    for view in [&first, &second] {
        assert_eq!(
            opened.read(view, "index.html").await.unwrap_err(),
            PackageErrorCode::ContentNotFound
        );
    }

    // Views of the new generation end when the Instance is disposed.
    let replacement = Opened {
        mount: open_mount(&opened.host).await.0,
        host: opened.host.clone(),
        data: opened.data.clone(),
        fixture: opened.fixture,
        next: std::cell::Cell::new(100),
    };
    let (third, _) = replacement
        .open("index.html", single(&replacement))
        .await
        .unwrap();
    assert!(replacement.read(&third, "index.html").await.is_ok());
    let instance = replacement.host.snapshot().await.instances[0].key.clone();
    replacement
        .host
        .execute(PackageHostCommand::Dispose(DisposePackage { instance }))
        .await
        .unwrap();
    assert_eq!(
        replacement.read(&third, "index.html").await.unwrap_err(),
        PackageErrorCode::ContentNotFound
    );
}

#[tokio::test]
async fn a_declaring_packages_mounts_report_nested_content() {
    let opened = Opened::new().await;
    assert!(open_mount(&opened.host).await.1);
}
