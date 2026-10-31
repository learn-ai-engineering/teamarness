//! The Exhibit fixtures the test-only Exhibits Consumer uploads.
//!
//! - `revision-1`: the viewer spike's two-page fixture (inline script and style, a relative
//!   stylesheet with `@import`, PNG and SVG images, static and dynamic ES modules, `fetch` of its
//!   own JSON and WebAssembly). Each page shows its checks as visible text and a `PASS` or `FAIL`
//!   verdict; page 1 navigates to page 2, which navigates back with its verdict.
//! - `revision-2`: revision 1 with a visible change on the entry page; every other file is
//!   byte-identical, so publishing it uploads one file.
//! - `media`: two pages and a 6-second H.264/AAC `mp4` (341,405 bytes, three chunks). The clip is
//!   synthetic and free of third-party content: it was generated with FFmpeg 9.0.1 from the
//!   `testsrc2` and `sine` lavfi sources (`-c:v libx264 -preset slow -profile:v baseline
//!   -pix_fmt yuv420p -b:v 400k -g 24 -c:a aac -b:a 64k -movflags +faststart -map_metadata -1
//!   -fflags +bitexact`).
//! - `isolation-probe`: a content page that checks it is opaque-origin, sees no Tauri globals,
//!   cannot read `parent` or `top`, gets no reply to a forged bridge connect, and attempts
//!   message-handler and `ipc:` posts that must create no Workspace. It reports in visible text;
//!   with the `#unsandboxed` fragment it labels itself as a frame created without a `sandbox`
//!   attribute.

/// One file of a fixture: its path inside the Exhibit and its bytes.
pub type FixtureFile = (&'static str, &'static [u8]);

macro_rules! file {
    ($fixture:literal, $path:literal) => {
        (
            $path,
            include_bytes!(concat!("../fixtures/exhibits/", $fixture, "/", $path)).as_slice(),
        )
    };
}

/// The fixture names [`fixture`] accepts.
pub const FIXTURES: &[&str] = &["revision-1", "revision-2", "media", "isolation-probe"];

const SHARED: &[FixtureFile] = &[
    file!("revision-1", "assets/data.json"),
    file!("revision-1", "assets/dot.png"),
    file!("revision-1", "assets/more.css"),
    file!("revision-1", "assets/shape.svg"),
    file!("revision-1", "assets/style.css"),
    file!("revision-1", "js/common.mjs"),
    file!("revision-1", "js/dynamic.mjs"),
    file!("revision-1", "js/lib.mjs"),
    file!("revision-1", "js/main.mjs"),
    file!("revision-1", "js/page2.mjs"),
    file!("revision-1", "pages/page2.html"),
    file!("revision-1", "wasm/add.wasm"),
];

const MEDIA: &[FixtureFile] = &[
    file!("media", "index.html"),
    file!("media", "pages/notes.html"),
    file!("media", "assets/media.css"),
    file!("media", "assets/clip.mp4"),
];

const ISOLATION_PROBE: &[FixtureFile] = &[
    file!("isolation-probe", "index.html"),
    file!("isolation-probe", "probe.js"),
];

/// A fixture's files, entry page first, or `None` for an unknown name.
#[must_use]
pub fn fixture(name: &str) -> Option<Vec<FixtureFile>> {
    let entry = match name {
        "revision-1" => file!("revision-1", "index.html"),
        "revision-2" => file!("revision-2", "index.html"),
        "media" => return Some(MEDIA.to_vec()),
        "isolation-probe" => return Some(ISOLATION_PROBE.to_vec()),
        _ => return None,
    };
    let mut files = vec![entry];
    files.extend_from_slice(SHARED);
    Some(files)
}

/// The default Exhibit title of a fixture.
#[must_use]
pub fn title(name: &str) -> &'static str {
    match name {
        "revision-1" => "Exhibit fixture (revision 1)",
        "revision-2" => "Exhibit fixture (revision 2)",
        "media" => "Exhibit media fixture",
        "isolation-probe" => "Exhibit isolation probe",
        _ => "Exhibit fixture",
    }
}
