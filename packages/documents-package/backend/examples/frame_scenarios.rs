//! The Markdown authority for the rendered-editor frame scenarios
//! (`packages/documents-package/ui/frame-scenarios/`, run by `npm run verify:documents-editor`).
//!
//! The harness relays Installations through an in-memory record log in Node, but the Backend's
//! Markdown core stays the only authority on Canonical Markdown and on Markdown revisions, so the
//! harness asks this program:
//!
//! - `genesis <peer>`: writes the body genesis (one empty paragraph) as one update to stdout.
//! - `export <body>`: writes the Canonical Markdown of the body (a Loro snapshot or updates file)
//!   to stdout. A body that is not a valid tree under the shared schema exits with status 2.
//! - `revise <body> <markdown> <peer>`: merges the revision in the `<markdown>` file against the
//!   body's current version as `<peer>`, as `document.revise` does, and writes the update.

use std::io::Write;
use std::process::ExitCode;

use loro::{ExportMode, LoroDoc};
use teamarness_documents_package::markdown;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(output) => {
            let mut stdout = std::io::stdout().lock();
            if stdout
                .write_all(&output)
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err((code, message)) => {
            eprintln!("frame_scenarios: {message}");
            ExitCode::from(code)
        }
    }
}

type Failure = (u8, String);

fn run(args: &[String]) -> Result<Vec<u8>, Failure> {
    match args {
        [command, peer] if command == "genesis" => {
            let doc = LoroDoc::new();
            doc.set_peer_id(parse_peer(peer)?).map_err(usage)?;
            markdown::write_tree(&doc, &[]).map_err(|error| (1, error.to_string()))?;
            doc.export(ExportMode::all_updates()).map_err(usage)
        }
        [command, body] if command == "export" => {
            let doc = open(body)?;
            markdown::export(&doc)
                .map(String::into_bytes)
                .map_err(|violation| (2, violation.to_string()))
        }
        [command, body, revision, peer] if command == "revise" => {
            let doc = open(body)?;
            let text = std::fs::read_to_string(revision).map_err(usage)?;
            let merged = markdown::merge(&doc, &doc.oplog_frontiers(), &text, parse_peer(peer)?)
                .map_err(|error| (2, error.to_string()))?;
            Ok(merged.update)
        }
        _ => Err((
            64,
            "usage: genesis <peer> | export <body> | revise <body> <markdown> <peer>".to_owned(),
        )),
    }
}

fn open(path: &str) -> Result<LoroDoc, Failure> {
    let bytes = std::fs::read(path).map_err(usage)?;
    let doc = LoroDoc::new();
    doc.import(&bytes).map_err(usage)?;
    Ok(doc)
}

fn parse_peer(text: &str) -> Result<u64, Failure> {
    text.parse()
        .map_err(|_| (64, format!("not a peer: {text}")))
}

fn usage(error: impl std::fmt::Display) -> Failure {
    (1, error.to_string())
}
