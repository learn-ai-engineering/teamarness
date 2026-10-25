//! Offline qualification only: mock runtime, no windows, transfers, or installer calls.
use semver::Version;
use serde_json::{Value, json};
use std::{error::Error, fs::File, io::Read, path::Path};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const FEED_LIMIT: u64 = 64 * 1024;
const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;
const KEY_LIMIT: u64 = 4 * 1024;

fn stable_version(value: &str) -> Result<Version> {
    let version = Version::parse(value)?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != value {
        return Err("expected canonical stable MAJOR.MINOR.PATCH".into());
    }
    Ok(version)
}

fn qualify(manifest: Value, bytes: &[u8], key: &str, tag: &str, target: &str) -> Result<()> {
    if bytes.len() as u64 > ARCHIVE_LIMIT || key.len() as u64 > KEY_LIMIT {
        return Err("qualification input exceeds its byte limit".into());
    }
    let expected = stable_version(
        tag.strip_prefix('v')
            .ok_or("expected vMAJOR.MINOR.PATCH tag")?,
    )?;
    let announced = manifest["version"]
        .as_str()
        .ok_or("manifest version missing")?;
    if stable_version(announced)? != expected {
        return Err("manifest version differs from independently expected tag".into());
    }
    if !matches!(target, "darwin-aarch64" | "darwin-aarch64-app") {
        return Err("expected darwin-aarch64 or darwin-aarch64-app target".into());
    }
    // Require a static manifest entry, not the plugin's dynamic-manifest fallback.
    let platform = &manifest["platforms"][target];
    let expected_url = platform["url"]
        .as_str()
        .ok_or("expected target URL missing")?
        .to_owned();
    let expected_signature = platform["signature"]
        .as_str()
        .ok_or("expected target signature missing")?
        .to_owned();

    let mut context = mock_context(noop_assets());
    // Qualification may inspect the first release. Keep the official newer-only comparator
    // and use an explicitly synthetic installed version, never a production override.
    context.package_info_mut().version = Version::new(0, 0, 0);
    context.config_mut().plugins.0.insert("updater".into(), json!({
        "pubkey": key.trim(),
        "requireSignedVersion": true,
        "endpoints": ["https://github.com/learn-ai-engineering/teamarness/releases/latest/download/latest.json"],
    }));
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)?;
    let update = app
        .updater_builder()
        .target(target)
        .build()?
        .restore_update(manifest)?
        .ok_or("manifest is not newer than synthetic installed 0.0.0")?;
    if update.version != expected.to_string()
        || update.target != target
        || update.download_url.as_str() != expected_url
        || update.signature != expected_signature
    {
        return Err("official updater selected unexpected version or target entry".into());
    }
    update.verify(bytes)?;
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("qualification inputs must be regular files".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("qualification file exceeds its byte limit".into());
    }
    Ok(bytes)
}

const HELP: &str = "Offline signed-version qualification (no GUI, network, or installation).
Usage: qualify-update --manifest FILE --archive FILE --public-key FILE --tag vX.Y.Z --target TARGET
TARGET: darwin-aarch64 or darwin-aarch64-app. The tag is an independent expected stable version.
Regular-file limits: manifest 64 KiB, archive 512 MiB, public key 4 KiB.
Uses the pinned official restore_update + Update::verify with synthetic installed version 0.0.0.
Only public keys are accepted here. Never supply a private key.";

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return Ok(());
    }
    let mut inputs = std::collections::HashMap::new();
    for pair in args.chunks(2) {
        if pair.len() != 2
            || !matches!(
                pair[0].as_str(),
                "--manifest" | "--archive" | "--public-key" | "--tag" | "--target"
            )
            || inputs.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("invalid or duplicate arguments; use --help".into());
        }
    }
    if inputs.len() != 5 {
        return Err("five named inputs required; use --help".into());
    }
    let manifest = read_bounded(Path::new(inputs["--manifest"]), FEED_LIMIT)?;
    let bytes = read_bounded(Path::new(inputs["--archive"]), ARCHIVE_LIMIT)?;
    let key = String::from_utf8(read_bounded(Path::new(inputs["--public-key"]), KEY_LIMIT)?)?;
    qualify(
        serde_json::from_slice(&manifest)?,
        &bytes,
        &key,
        inputs["--tag"],
        inputs["--target"],
    )?;
    println!(
        "Verified signed bytes for {} ({})",
        inputs["--tag"], inputs["--target"]
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qualification failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BYTES: &[u8] = include_bytes!("../tests/fixtures/updater/artifact.bin");
    const KEY: &str = include_str!("../tests/fixtures/updater/public-key.txt");
    const SIGNATURE: &str = include_str!("../tests/fixtures/updater/versioned.sig");
    const TARGETS: [&str; 2] = ["darwin-aarch64", "darwin-aarch64-app"];

    fn manifest(version: &str, signature: &str) -> Value {
        let platform = json!({
            "url": "https://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/Teamarness.app.tar.gz",
            "signature": signature.trim(),
        });
        json!({"version": version, "platforms": {
            "darwin-aarch64": platform,
            "darwin-aarch64-app": platform,
        }})
    }

    // Verifies: TEAM.CONTRIB-R02. ADR 0031 requires official signed-version verification.
    #[test]
    fn genuine_signed_bytes_verify_for_both_macos_targets() {
        for target in TARGETS {
            qualify(manifest("1.2.3", SIGNATURE), BYTES, KEY, "v1.2.3", target).unwrap();
        }
    }

    #[test]
    fn altered_bytes_never_verify() {
        let mut altered = BYTES.to_vec();
        altered[0] ^= 1;
        for target in TARGETS {
            assert!(
                qualify(
                    manifest("1.2.3", SIGNATURE),
                    &altered,
                    KEY,
                    "v1.2.3",
                    target
                )
                .is_err()
            );
        }
    }

    #[test]
    fn inflated_announcement_with_genuine_older_signature_never_verifies() {
        for target in TARGETS {
            let error =
                qualify(manifest("9.9.9", SIGNATURE), BYTES, KEY, "v9.9.9", target).unwrap_err();
            assert!(
                matches!(error.downcast_ref::<tauri_plugin_updater::Error>(),
                Some(tauri_plugin_updater::Error::SignedVersionMismatch { signed, announced })
                    if signed == "1.2.3" && announced == "9.9.9")
            );
        }
    }

    #[test]
    fn missing_signed_version_never_verifies() {
        let signature = include_str!("../tests/fixtures/updater/unversioned.sig");
        for target in TARGETS {
            let error =
                qualify(manifest("1.2.3", signature), BYTES, KEY, "v1.2.3", target).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<tauri_plugin_updater::Error>(),
                Some(tauri_plugin_updater::Error::MissingSignedVersion)
            ));
        }
    }

    #[test]
    fn changed_trusted_comment_never_verifies_even_when_announced_version_matches() {
        let signature = include_str!("../tests/fixtures/updater/changed-comment.sig");
        for target in TARGETS {
            let error =
                qualify(manifest("9.9.9", signature), BYTES, KEY, "v9.9.9", target).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<tauri_plugin_updater::Error>(),
                Some(tauri_plugin_updater::Error::Minisign(_))
            ));
        }
    }

    #[test]
    fn tag_is_an_independent_expectation() {
        assert!(
            qualify(
                manifest("1.2.3", SIGNATURE),
                BYTES,
                KEY,
                "v1.2.4",
                TARGETS[0]
            )
            .is_err()
        );
    }

    #[test]
    fn noncanonical_versions_and_tags_are_rejected() {
        for version in ["v1.2.3", "01.2.3", "1.2.3-beta.1", "1.2.3+build", "1.2"] {
            assert!(
                qualify(
                    manifest(version, SIGNATURE),
                    BYTES,
                    KEY,
                    &format!("v{version}"),
                    TARGETS[0]
                )
                .is_err()
            );
        }
        for tag in ["1.2.3", "vv1.2.3", "v01.2.3", "v1.2.3+build"] {
            assert!(qualify(manifest("1.2.3", SIGNATURE), BYTES, KEY, tag, TARGETS[0]).is_err());
        }
    }

    #[test]
    fn file_inputs_accept_the_limit_and_reject_one_extra_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input");
        std::fs::write(&path, b"1234").unwrap();
        assert_eq!(read_bounded(&path, 4).unwrap(), b"1234");
        assert!(read_bounded(&path, 3).is_err());
        assert!(read_bounded(directory.path(), 4).is_err());
    }

    #[test]
    fn synthetic_installed_version_still_requires_a_newer_release() {
        assert!(
            qualify(
                manifest("0.0.0", SIGNATURE),
                BYTES,
                KEY,
                "v0.0.0",
                TARGETS[0]
            )
            .is_err()
        );
    }

    #[test]
    fn each_target_selects_its_own_signature_and_requires_its_entry() {
        for target in TARGETS {
            let other = TARGETS.into_iter().find(|name| *name != target).unwrap();
            let mut metadata = manifest("1.2.3", SIGNATURE);
            metadata["platforms"][other]["signature"] = json!("invalid signature");
            qualify(metadata.clone(), BYTES, KEY, "v1.2.3", target).unwrap();
            assert!(qualify(metadata.clone(), BYTES, KEY, "v1.2.3", other).is_err());
            metadata["platforms"]
                .as_object_mut()
                .unwrap()
                .remove(target);
            assert!(qualify(metadata, BYTES, KEY, "v1.2.3", target).is_err());
        }
        assert!(
            qualify(
                manifest("1.2.3", SIGNATURE),
                BYTES,
                KEY,
                "v1.2.3",
                "windows-x86_64"
            )
            .is_err()
        );
    }
}
