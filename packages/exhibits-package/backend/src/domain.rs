//! The Exhibit rules every Port and UI request shares: identities, titles, content paths, the
//! upload chunking, the bounds and the `exhibits.*` error codes.
//!
//! Paths, extensions and size bounds come from `teamarness-package-contract`'s content rules,
//! the same code the Package Host applies when it opens a content view, so a file this Package
//! accepts is a file the Host will serve.

use serde_json::{Value as JsonValue, json};
use teamarness_package_contract::media_types::{
    MAX_CONTENT_FILE_BYTES, MAX_CONTENT_FILES, MAX_CONTENT_PATH_BYTES, MAX_CONTENT_TOTAL_BYTES,
    is_valid_content_path, media_type_for_path,
};
use teamarness_package_sdk::BackendError;

/// The raw size of every `file.put` chunk except the last (128 KiB).
pub const CHUNK_BYTES: usize = 131_072;
/// The largest file, in bytes.
pub const MAX_FILE_BYTES: u64 = MAX_CONTENT_FILE_BYTES;
/// The largest declared total of one Draft's files, in bytes.
pub const MAX_TOTAL_BYTES: u64 = MAX_CONTENT_TOTAL_BYTES;
/// The most files one Draft lists.
pub const MAX_FILES: usize = MAX_CONTENT_FILES;
/// The longest content path, in UTF-8 bytes.
pub const MAX_PATH_BYTES: usize = MAX_CONTENT_PATH_BYTES;
/// The most chunks one file can have.
pub const MAX_CHUNKS: u64 = MAX_FILE_BYTES.div_ceil(CHUNK_BYTES as u64);
/// The longest Exhibit ID, in characters.
pub const MAX_ID_CHARS: usize = 48;
/// The longest Exhibit title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// The most hashes one `files.missing` query names.
pub const MAX_MISSING_QUERY: usize = 256;
/// The most Exhibits one `exhibit.list` page returns.
pub const LIST_PAGE: usize = 64;
/// The most files one `exhibit.read` page returns. A full page at the path bound stays well
/// under the 256 KiB Port payload bound.
pub const READ_PAGE: usize = 500;
/// The largest `file.write` text, in UTF-8 bytes (96 KiB). Even when every character is a quote
/// or backslash that doubles when written as JSON, the request stays under the 256 KiB Port
/// payload bound. It is below [`CHUNK_BYTES`], so a written file is always one chunk.
pub const MAX_WRITE_BYTES: usize = 98_304;
/// The most `file.put` calls a Consumer should have in flight at once.
pub const MAX_CONCURRENT_PUTS: usize = 4;
/// How long a partial upload, or a complete file no Draft references, is kept (24 hours).
pub const GC_GRACE_MS: i64 = 24 * 60 * 60 * 1000;
/// The most missing hashes a `exhibits.missing_files` error lists; `files.missing` lists all.
/// It keeps the error inside the Host's 4 KiB bound on Package error details.
pub const MAX_MISSING_IN_ERROR: usize = 32;
/// The Exhibit ID pattern, as published in `runtime.guide` and `ports.json`.
pub const ID_PATTERN: &str = "^[a-z0-9]+(-[a-z0-9]+)*$";

/// Why a request was refused, as an `exhibits.*` code with bounded details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExhibitsError {
    /// `draft.update` names files the content store does not hold completely.
    MissingFiles(Vec<String>),
    /// `expectedRevision` does not match the Draft's current revision (0 when absent).
    RevisionConflict {
        current: u64,
    },
    InvalidId,
    InvalidTitle,
    InvalidPath {
        path: String,
        reason: &'static str,
    },
    TypeRefused {
        path: String,
    },
    TooLarge {
        limit: &'static str,
    },
    InvalidChunk {
        reason: &'static str,
    },
    /// The assembled file's size or SHA-256 differs from the declared one; nothing was kept.
    ContentMismatch,
    NotFound,
    InvalidRequest,
    ActorUnauthorized,
    Storage,
    ContentViewsUnavailable {
        cause: String,
    },
}

impl ExhibitsError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingFiles(_) => "exhibits.missing_files",
            Self::RevisionConflict { .. } => "exhibits.revision_conflict",
            Self::InvalidId => "exhibits.invalid_id",
            Self::InvalidTitle => "exhibits.invalid_title",
            Self::InvalidPath { .. } => "exhibits.invalid_path",
            Self::TypeRefused { .. } => "exhibits.type_refused",
            Self::TooLarge { .. } => "exhibits.too_large",
            Self::InvalidChunk { .. } => "exhibits.invalid_chunk",
            Self::ContentMismatch => "exhibits.content_mismatch",
            Self::NotFound => "exhibits.not_found",
            Self::InvalidRequest => "exhibits.invalid_request",
            Self::ActorUnauthorized => "exhibits.actor_unauthorized",
            Self::Storage => "exhibits.storage_failed",
            Self::ContentViewsUnavailable { .. } => "exhibits.content_views_unavailable",
        }
    }

    fn details(&self) -> JsonValue {
        match self {
            Self::MissingFiles(missing) => json!({
                "missing": missing.iter().take(MAX_MISSING_IN_ERROR).collect::<Vec<_>>(),
                "missingCount": missing.len(),
            }),
            Self::RevisionConflict { current } => json!({ "revision": current }),
            Self::InvalidId => json!({ "pattern": ID_PATTERN, "maxChars": MAX_ID_CHARS }),
            Self::InvalidTitle => json!({ "maxChars": MAX_TITLE_CHARS }),
            Self::InvalidPath { path, reason } => json!({ "path": path, "reason": reason }),
            Self::TypeRefused { path } => json!({ "path": path }),
            Self::TooLarge { limit } => json!({ "limit": limit }),
            Self::InvalidChunk { reason } => json!({ "reason": reason }),
            Self::ContentViewsUnavailable { cause } => json!({ "causeCode": cause }),
            _ => json!({}),
        }
    }

    const fn retryable(&self) -> bool {
        matches!(self, Self::Storage | Self::ContentViewsUnavailable { .. })
    }
}

impl From<ExhibitsError> for BackendError {
    fn from(error: ExhibitsError) -> Self {
        Self::new(error.code(), error.retryable(), error.details())
    }
}

/// Whether `id` is a valid Exhibit ID: lowercase kebab case, at most 48 characters.
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// # Errors
/// `exhibits.invalid_id` when the ID does not match [`ID_PATTERN`] within 48 characters.
pub fn validate_id(id: &str) -> Result<(), ExhibitsError> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(ExhibitsError::InvalidId)
    }
}

/// # Errors
/// `exhibits.invalid_title` for an empty or whitespace-only title, a control character, or more
/// than 120 characters.
pub fn validate_title(title: &str) -> Result<(), ExhibitsError> {
    if title.trim().is_empty()
        || title.chars().count() > MAX_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        Err(ExhibitsError::InvalidTitle)
    } else {
        Ok(())
    }
}

/// Checks one content path: the shared content path rules, no `"` (so a path never grows when
/// it is written as JSON, which keeps the largest request inside the Port bound), and an
/// extension on the shared allowlist.
///
/// # Errors
/// `exhibits.invalid_path` for a malformed path, or `exhibits.type_refused` for an extension
/// outside the allowlist. Extensions match lowercase only.
pub fn validate_path(path: &str) -> Result<(), ExhibitsError> {
    if !is_valid_content_path(path) || path.contains('"') {
        return Err(ExhibitsError::InvalidPath {
            path: bounded(path),
            reason: if path.len() > MAX_PATH_BYTES {
                "too_long"
            } else {
                "malformed"
            },
        });
    }
    if media_type_for_path(path).is_none() {
        return Err(ExhibitsError::TypeRefused {
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// A path cut to the path bound, for an error's details.
fn bounded(path: &str) -> String {
    let mut end = path.len().min(MAX_PATH_BYTES);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    path[..end].to_owned()
}

/// Whether `value` is a lowercase hexadecimal SHA-256.
#[must_use]
pub fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The number of chunks a file of `size` bytes is uploaded in. An empty file is one empty chunk.
#[must_use]
pub const fn chunk_count(size: u64) -> u64 {
    if size == 0 {
        1
    } else {
        size.div_ceil(CHUNK_BYTES as u64)
    }
}

/// The exact raw length of chunk `index` of a file of `size` bytes.
#[must_use]
pub const fn chunk_length(size: u64, index: u64) -> u64 {
    let start = index * CHUNK_BYTES as u64;
    let end = start + CHUNK_BYTES as u64;
    if end > size {
        size - start
    } else {
        CHUNK_BYTES as u64
    }
}

/// Validates a `file.put` header before its data is decoded.
///
/// # Errors
/// `exhibits.invalid_request` for a malformed hash, `exhibits.too_large` (`file`) for a size
/// over 16 MiB, and `exhibits.invalid_chunk` when the chunk count does not match the size or
/// the index is out of range.
pub fn validate_chunk_header(
    sha256: &str,
    size: u64,
    index: u64,
    count: u64,
) -> Result<(), ExhibitsError> {
    if !is_sha256(sha256) {
        return Err(ExhibitsError::InvalidRequest);
    }
    if size > MAX_FILE_BYTES {
        return Err(ExhibitsError::TooLarge { limit: "file" });
    }
    if count != chunk_count(size) {
        return Err(ExhibitsError::InvalidChunk {
            reason: "chunk_count",
        });
    }
    if index >= count {
        return Err(ExhibitsError::InvalidChunk {
            reason: "chunk_index",
        });
    }
    Ok(())
}

/// Formats Unix milliseconds as an RFC 3339 UTC timestamp with millisecond precision.
#[must_use]
pub fn rfc3339(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let of_day = ms.rem_euclid(86_400_000);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1_000 % 60,
        of_day % 1_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_lowercase_kebab_case_within_48_characters() {
        for valid in ["a", "q3-plan", "exhibit-2", "0", &"a".repeat(48)] {
            assert!(is_valid_id(valid), "{valid}");
        }
        for invalid in [
            "",
            "-a",
            "a-",
            "a--b",
            "A",
            "a_b",
            "a b",
            "é",
            &"a".repeat(49),
        ] {
            assert_eq!(
                validate_id(invalid),
                Err(ExhibitsError::InvalidId),
                "{invalid}"
            );
        }
    }

    #[test]
    fn titles_are_bounded_and_printable() {
        assert!(validate_title("Quarterly plan").is_ok());
        assert!(validate_title(&"é".repeat(120)).is_ok());
        for invalid in ["", "   ", "a\nb", &"x".repeat(121)] {
            assert_eq!(validate_title(invalid), Err(ExhibitsError::InvalidTitle));
        }
    }

    #[test]
    fn paths_use_the_shared_rules_and_allowlist() {
        assert!(validate_path("index.html").is_ok());
        assert!(validate_path("pages/page2.html").is_ok());
        for malformed in [
            "",
            "/a.html",
            "a/../b.html",
            "a//b.css",
            "a\\b.css",
            "q\"uote.css",
        ] {
            assert!(
                matches!(
                    validate_path(malformed),
                    Err(ExhibitsError::InvalidPath { .. })
                ),
                "{malformed}"
            );
        }
        let long = format!("{}.css", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(
            validate_path(&long),
            Err(ExhibitsError::InvalidPath {
                path: long[..MAX_PATH_BYTES].to_owned(),
                reason: "too_long"
            })
        );
        for refused in ["run.exe", "image.PNG", "LICENSE", "page.htm"] {
            assert_eq!(
                validate_path(refused),
                Err(ExhibitsError::TypeRefused {
                    path: refused.into()
                })
            );
        }
    }

    #[test]
    fn chunking_is_exact() {
        assert_eq!(chunk_count(0), 1);
        assert_eq!(chunk_length(0, 0), 0);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(CHUNK_BYTES as u64), 1);
        assert_eq!(chunk_count(CHUNK_BYTES as u64 + 1), 2);
        assert_eq!(chunk_length(CHUNK_BYTES as u64 + 1, 0), CHUNK_BYTES as u64);
        assert_eq!(chunk_length(CHUNK_BYTES as u64 + 1, 1), 1);
        assert_eq!(chunk_count(MAX_FILE_BYTES), MAX_CHUNKS);
        assert_eq!(MAX_CHUNKS, 128);
        const { assert!(MAX_WRITE_BYTES <= CHUNK_BYTES) };
        let sha = "a".repeat(64);
        assert!(validate_chunk_header(&sha, 10, 0, 1).is_ok());
        assert_eq!(
            validate_chunk_header(&sha, MAX_FILE_BYTES + 1, 0, MAX_CHUNKS + 1),
            Err(ExhibitsError::TooLarge { limit: "file" })
        );
        assert_eq!(
            validate_chunk_header(&sha, 10, 0, 2),
            Err(ExhibitsError::InvalidChunk {
                reason: "chunk_count"
            })
        );
        assert_eq!(
            validate_chunk_header(&sha, 10, 1, 1),
            Err(ExhibitsError::InvalidChunk {
                reason: "chunk_index"
            })
        );
        assert_eq!(
            validate_chunk_header("ABC", 10, 0, 1),
            Err(ExhibitsError::InvalidRequest)
        );
    }

    #[test]
    fn missing_files_details_stay_inside_the_host_bound() {
        let missing: Vec<String> = (0..1000).map(|i| format!("{i:064x}")).collect();
        let error: BackendError = ExhibitsError::MissingFiles(missing).into();
        assert_eq!(error.code(), "exhibits.missing_files");
        assert_eq!(error.details()["missingCount"], 1000);
        assert_eq!(
            error.details()["missing"].as_array().unwrap().len(),
            MAX_MISSING_IN_ERROR
        );
        assert!(serde_json::to_vec(error.details()).unwrap().len() <= 4096);
    }

    #[test]
    fn timestamps_format_as_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_793_000_000_123), "2026-10-26T07:33:20.123Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
