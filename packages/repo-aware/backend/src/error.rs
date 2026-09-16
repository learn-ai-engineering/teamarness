use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorInfo {
    pub code: &'static str,
    pub retryable: bool,
}

pub const PATH_MISSING: ErrorInfo = ErrorInfo {
    code: "repo_aware.path_missing",
    retryable: false,
};
pub const PATH_NOT_ABSOLUTE: ErrorInfo = ErrorInfo {
    code: "repo_aware.path_not_absolute",
    retryable: false,
};
pub const NOT_DIRECTORY: ErrorInfo = ErrorInfo {
    code: "repo_aware.not_directory",
    retryable: false,
};
pub const NOT_GIT_REPOSITORY: ErrorInfo = ErrorInfo {
    code: "repo_aware.not_git_repository",
    retryable: false,
};
pub const REPOSITORY_UNBORN: ErrorInfo = ErrorInfo {
    code: "repo_aware.repository_unborn",
    retryable: false,
};
pub const DUPLICATE_REGISTRATION: ErrorInfo = ErrorInfo {
    code: "repo_aware.duplicate_registration",
    retryable: false,
};
pub const REGISTRATION_NOT_FOUND: ErrorInfo = ErrorInfo {
    code: "repo_aware.registration_not_found",
    retryable: false,
};
pub const STORAGE_FAILED: ErrorInfo = ErrorInfo {
    code: "repo_aware.storage_failed",
    retryable: true,
};
pub const REPOSITORY_UNAVAILABLE: ErrorInfo = ErrorInfo {
    code: "repo_aware.repository_unavailable",
    retryable: true,
};
pub const SNAPSHOT_INTERRUPTED: ErrorInfo = ErrorInfo {
    code: "repo_aware.snapshot_interrupted",
    retryable: true,
};
pub const INVALID_REQUEST: ErrorInfo = ErrorInfo {
    code: "repo_aware.invalid_request",
    retryable: false,
};

impl From<ErrorInfo> for teamarness_package_sdk::BackendError {
    fn from(info: ErrorInfo) -> Self {
        Self::new(info.code, info.retryable, teamarness_package_protocol::JsonValue::Object(Default::default()))
    }
}