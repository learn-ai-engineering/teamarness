//! The `runtime.guide` answer: what an Agent needs to know to author an Exhibit, as Markdown
//! and as data. Every number in it is the constant the Backend enforces.

use serde_json::{Map, Value as JsonValue, json};
use teamarness_package_contract::media_types::MEDIA_TYPES;

use crate::domain::{
    CHUNK_BYTES, ID_PATTERN, LIST_PAGE, MAX_CONCURRENT_PUTS, MAX_FILE_BYTES, MAX_FILES,
    MAX_ID_CHARS, MAX_MISSING_QUERY, MAX_PATH_BYTES, MAX_TITLE_CHARS, MAX_TOTAL_BYTES,
    MAX_WRITE_BYTES, READ_PAGE,
};

/// The enforced limits, as `runtime.guide` publishes them.
#[must_use]
pub fn limits() -> JsonValue {
    json!({
        "chunkBytes": CHUNK_BYTES,
        "maxFileBytes": MAX_FILE_BYTES,
        "maxTotalBytes": MAX_TOTAL_BYTES,
        "maxFiles": MAX_FILES,
        "maxPathBytes": MAX_PATH_BYTES,
        "maxIdChars": MAX_ID_CHARS,
        "maxTitleChars": MAX_TITLE_CHARS,
        "maxConcurrentPuts": MAX_CONCURRENT_PUTS,
    })
}

/// The allowed extensions and the media type each is served with.
#[must_use]
pub fn media_types() -> JsonValue {
    JsonValue::Object(
        MEDIA_TYPES
            .iter()
            .map(|(extension, media_type)| ((*extension).to_owned(), json!(media_type)))
            .collect::<Map<_, _>>(),
    )
}

/// The Agent-facing Markdown.
#[must_use]
pub fn markdown() -> String {
    let extensions = MEDIA_TYPES
        .iter()
        .map(|(extension, _)| format!("`.{extension}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mib = |bytes: u64| bytes / (1024 * 1024);
    format!(
        "# Authoring an Exhibit

An Exhibit is a small static website: HTML pages with their CSS, JavaScript, images, fonts, \
media and data files. A Person opens it inside Teamarness in an isolated frame. You write it as \
a Draft and can replace the Draft as often as you like.

## Rules for content

- Bundle every file the Exhibit uses. Do not load anything from a CDN or another site: \
network access is not part of the Exhibit runtime and may be blocked at any time.
- Link pages and assets with relative URLs (`assets/style.css`, `../index.html`). Absolute \
paths starting with `/` and links to other Exhibits do not work.
- One listed file is the entry page, normally `index.html`. Navigation between your own pages, \
CSS `@import`, ES modules (static and dynamic `import`), `fetch` of your own files and \
WebAssembly all work.
- Not supported: Web Workers, nested frames, forms that submit, browser storage, popups and \
any access to the Teamarness window around the Exhibit.
- Allowed extensions: {extensions}. Extensions match **lowercase only**: `photo.png` is \
accepted and `photo.PNG` is refused with `exhibits.type_refused`.
- Paths are relative, `/`-separated, at most {MAX_PATH_BYTES} bytes, with no `.` or `..` \
segment and no `\\`, `%`, `?`, `#` or `\"`.
- Limits: at most {MAX_FILES} files, {max_file} MiB per file and {max_total} MiB in total.
- Keep library files (frameworks, fonts) byte-for-byte identical between revisions and \
Exhibits: content is stored by SHA-256, so an unchanged file is never uploaded twice.

## Storing files

Write text files with `file.write`. Send binary files with `file.put`.

- **Text files** (HTML, CSS, JavaScript, SVG, JSON, Markdown, plain text): call `file.write` \
with the whole file as `text`. It returns the file's `sha256` and `size`; use that `sha256` \
in `draft.update`. If you are a model, always use `file.write` for text: never compute a \
SHA-256 or base64 yourself. `text` is at most {MAX_WRITE_BYTES} bytes of UTF-8 \
(`exhibits.too_large` otherwise), so split larger code or data into several files. Writing \
the same text again is safe and returns the same `sha256`.
- **Binary files** (images, fonts, media, WebAssembly), or any file a program uploads:
   1. Compute its SHA-256 (lowercase hex) and size.
   2. Call `files.missing` with up to {MAX_MISSING_QUERY} hashes. Upload only what it reports.
   3. Upload it with `file.put` in chunks of exactly {CHUNK_BYTES} raw bytes (standard \
padded base64 in `data`); only the last chunk may be shorter. `chunkCount` is \
`ceil(size / {CHUNK_BYTES})`, and 1 for an empty file. Chunks may arrive in any order and may \
be repeated. The file is verified when its last missing chunk arrives, and `complete` becomes \
true. Keep at most {MAX_CONCURRENT_PUTS} `file.put` calls in flight.

Then call `draft.update` with the Exhibit ID, a title, the entry path and every file as \
`{{path, sha256}}`. The media type comes from the path's extension. It creates the Draft or \
replaces it completely, and returns the new `revision`. Pass the revision you last saw as \
`expectedRevision` to avoid overwriting someone else's update (`exhibits.revision_conflict` \
carries the current revision). If `draft.update` fails with `exhibits.missing_files`, store \
the files it names (at most 32 are listed; `missingCount` says how many) and call it again.

Over a Port, every refusal arrives as `provider_error`: the `exhibits.*` code is in \
`details.provider.code`, and its details (such as `missing` and `missingCount`) are in \
`details.provider.details`.

Exhibit IDs match `{ID_PATTERN}`, at most {MAX_ID_CHARS} characters. Titles are at most \
{MAX_TITLE_CHARS} characters.

## When an outcome is uncertain

A command may fail with `outcome_unknown`: it may or may not have taken effect. Never assume \
either. After an uncertain `file.put`, call `files.missing` and send only the chunks it \
reports. If a re-sent chunk fails with `exhibits.content_mismatch`, the chunks received did \
not form the file and were discarded: upload the whole file again. After an uncertain \
`file.write`, call it again with the same text. After an uncertain \
`draft.update`, call `exhibit.read` and compare the revision and files before trying again.

## Reading back

- `exhibit.list` returns Exhibits newest first, {LIST_PAGE} per page; pass `nextBefore` back \
as `before` for the next page.
- `exhibit.read` returns the Draft's entry, title, revision and files, {READ_PAGE} files per \
page; pass `nextOffset` back as `offset`.
- `file.read` returns one chunk of a stored file, so you can revise content you did not keep.
- `draft.delete` removes a Draft. Its files are kept for 24 hours, so recreating it soon \
needs no upload.
",
        max_file = mib(MAX_FILE_BYTES),
        max_total = mib(MAX_TOTAL_BYTES),
    )
}

/// The complete `runtime.guide` answer.
#[must_use]
pub fn guide() -> JsonValue {
    json!({
        "markdown": markdown(),
        "limits": limits(),
        "mediaTypes": media_types(),
        "idPattern": ID_PATTERN,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamarness_package_contract::media_types::{
        MAX_CONTENT_FILE_BYTES, MAX_CONTENT_FILES, MAX_CONTENT_PATH_BYTES, MAX_CONTENT_TOTAL_BYTES,
        media_type_for_extension,
    };

    #[test]
    fn limits_are_the_enforced_constants() {
        let guide = guide();
        let limits = &guide["limits"];
        assert_eq!(limits["chunkBytes"], 131_072);
        assert_eq!(limits["chunkBytes"], CHUNK_BYTES);
        assert_eq!(limits["maxFileBytes"], MAX_CONTENT_FILE_BYTES);
        assert_eq!(limits["maxTotalBytes"], MAX_CONTENT_TOTAL_BYTES);
        assert_eq!(limits["maxFiles"], MAX_CONTENT_FILES);
        assert_eq!(limits["maxPathBytes"], MAX_CONTENT_PATH_BYTES);
        assert_eq!(limits["maxIdChars"], MAX_ID_CHARS);
        assert_eq!(limits["maxTitleChars"], MAX_TITLE_CHARS);
        assert_eq!(limits["maxConcurrentPuts"], MAX_CONCURRENT_PUTS);
        assert_eq!(guide["idPattern"], ID_PATTERN);
    }

    #[test]
    fn media_types_are_the_shared_allowlist() {
        let guide = guide();
        let published = guide["mediaTypes"].as_object().unwrap();
        assert_eq!(published.len(), MEDIA_TYPES.len());
        for (extension, media_type) in MEDIA_TYPES {
            assert_eq!(published[*extension], *media_type);
            assert_eq!(media_type_for_extension(extension), Some(*media_type));
        }
    }

    #[test]
    fn markdown_states_the_runtime_rules() {
        let markdown = markdown();
        for needle in [
            "Bundle every file",
            "CDN",
            "relative URLs",
            "entry page",
            "lowercase only",
            "`photo.PNG` is refused",
            "1000 files",
            "16 MiB per file",
            "64 MiB in total",
            "131072 raw bytes",
            "always use `file.write` for text",
            "never compute a SHA-256 or base64 yourself",
            "at most 98304 bytes of UTF-8",
            "Binary files",
            "at most 4 `file.put` calls",
            "files.missing",
            "outcome_unknown",
            "arrives as `provider_error`",
            "`details.provider.code`",
            "`details.provider.details`",
            "Web Workers, nested frames",
            "`.html`",
            "`.wasm`",
            "`.mp4`",
        ] {
            assert!(markdown.contains(needle), "the guide must say {needle:?}");
        }
    }
}
