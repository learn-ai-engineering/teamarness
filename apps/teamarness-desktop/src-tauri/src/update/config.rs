use std::path::Path;

pub(super) const ENDPOINT: &str =
    "https://github.com/learn-ai-engineering/teamarness/releases/latest/download/latest.json";
pub(super) const FEED_LIMIT: usize = 64 * 1024;
pub(super) const ARCHIVE_LIMIT: usize = 512 * 1024 * 1024;

pub(super) fn build_enabled(debug: bool, opted_in: bool, production: bool, target: &str) -> bool {
    !debug && opted_in && production && target == "aarch64-apple-darwin"
}

pub(super) fn enabled() -> bool {
    build_enabled(
        cfg!(debug_assertions),
        cfg!(feature = "production-release"),
        env!("TEAMARNESS_PRODUCTION_RELEASE") == "true",
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else {
            "unsupported"
        },
    ) && std::env::current_exe().is_ok_and(|path| installed_writable(&path))
}

fn installed_writable(executable: &Path) -> bool {
    let Ok(executable) = executable.canonicalize() else {
        return false;
    };
    let Some(macos) = executable.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    let Some(bundle) = contents.parent() else {
        return false;
    };
    if macos.file_name().is_none_or(|name| name != "MacOS")
        || contents.file_name().is_none_or(|name| name != "Contents")
        || bundle
            .extension()
            .is_none_or(|extension| extension != "app")
        || executable.components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some("Volumes" | "AppTranslocation" | "target")
            )
        })
    {
        return false;
    }
    // Replacement requires a writable parent, not merely a writable executable.
    // A create-new probe honors ACLs/read-only mounts without changing bundle bytes.
    let Some(parent) = bundle.parent() else {
        return false;
    };
    let probe = parent.join(format!(".teamarness-update-write-{}", std::process::id()));
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    else {
        return false;
    };
    drop(file);
    std::fs::remove_file(probe).is_ok()
        && [bundle, contents, macos].iter().all(|path| {
            path.metadata()
                .is_ok_and(|metadata| !metadata.permissions().readonly())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_configuration_defaults_disabled_independent_of_runtime_environment() {
        for (debug, opt, production, target) in [
            (true, true, true, "aarch64-apple-darwin"),
            (true, false, false, "aarch64-apple-darwin"),
            (false, false, false, "aarch64-apple-darwin"),
            (false, true, false, "aarch64-apple-darwin"),
            (false, true, true, "x86_64-apple-darwin"),
        ] {
            assert!(!build_enabled(debug, opt, production, target));
        }
        assert!(build_enabled(false, true, true, "aarch64-apple-darwin"));
        assert!(
            !enabled(),
            "debug tests cannot opt into the production service"
        );
        let directory = tempfile::tempdir().unwrap();
        let exe = directory
            .path()
            .join("Teamarness.app/Contents/MacOS/Teamarness");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"test").unwrap();
        assert!(installed_writable(&exe));
        assert!(!installed_writable(directory.path()));
        let target = directory
            .path()
            .join("target/Teamarness.app/Contents/MacOS/Teamarness");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"test").unwrap();
        assert!(!installed_writable(&target));
    }
}
