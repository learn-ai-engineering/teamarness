use crate::error::{self, ErrorInfo};

pub fn validate_path(path: &str) -> Result<String, ErrorInfo> {
    if path.is_empty() {
        return Err(error::PATH_MISSING);
    }

    let canonical = std::fs::canonicalize(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => error::PATH_MISSING,
        std::io::ErrorKind::PermissionDenied => error::PATH_MISSING,
        _ => error::PATH_MISSING,
    })?;

    if !canonical.is_absolute() {
        return Err(error::PATH_NOT_ABSOLUTE);
    }

    if !canonical.is_dir() {
        return Err(error::NOT_DIRECTORY);
    }

    Ok(canonical.to_string_lossy().to_string())
}

pub fn validate_git_checkout(path: &str) -> Result<(), ErrorInfo> {
    let repo = gix::open(path).map_err(|_| error::NOT_GIT_REPOSITORY)?;

    repo.workdir()
        .ok_or(error::NOT_GIT_REPOSITORY)?;

    let mut head = repo.head().map_err(|_| error::NOT_GIT_REPOSITORY)?;
    if head.peel_to_commit().is_err() {
        return Err(error::REPOSITORY_UNBORN);
    }

    Ok(())
}

pub fn git_root(path: &str) -> Result<String, ErrorInfo> {
    let repo = gix::discover(path).map_err(|_| error::NOT_GIT_REPOSITORY)?;
    repo.workdir()
        .map(|p| {
            std::fs::canonicalize(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_string()
        })
        .ok_or(error::NOT_GIT_REPOSITORY)
}