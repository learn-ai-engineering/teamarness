//! Runs every row of the shared `fixtures/merge-scenarios.json` against `markdown::merge`.
//! The Documents port system test runs the same rows through `document.revise`.

use std::path::Path;

use loro::{ExportMode, LoroDoc};
use serde::Deserialize;
use teamarness_documents_package::markdown::{export, merge, read_tree, write_tree};

mod common;
use common::{AGENT, HUMAN, human_types, peer_copy, sync};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Table {
    #[allow(dead_code)]
    description: String,
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scenario {
    name: String,
    initial: String,
    base: Base,
    concurrent_human_edits: Vec<HumanEdit>,
    revision: String,
    expected_markdown: String,
    expected_ops: Option<usize>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum Base {
    Initial,
    Current,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HumanEdit {
    block: usize,
    at: At,
    text: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
enum At {
    Start,
    End,
}

/// The peer that turns the genesis into `initial`, as the port's first revise would.
const SETUP: u64 = 0x5E70;

fn load() -> Table {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/merge-scenarios.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn run(scenario: &Scenario) -> Result<(), String> {
    let server = LoroDoc::new();
    server.set_peer_id(1).unwrap();
    write_tree(&server, &[]).unwrap();
    let setup = merge(&server, &server.oplog_frontiers(), &scenario.initial, SETUP)
        .map_err(|e| format!("initial: {e}"))?;
    server.import(&setup.update).unwrap();
    let initial = server.oplog_frontiers();
    if export(&server).unwrap() != scenario.initial {
        return Err("`initial` is not Canonical Markdown".into());
    }

    let human = peer_copy(&server, HUMAN);
    for edit in &scenario.concurrent_human_edits {
        human_types(&human, edit.block, edit.at == At::End, &edit.text);
    }
    server
        .import(&human.export(ExportMode::all_updates()).unwrap())
        .unwrap();
    let base = match scenario.base {
        Base::Initial => initial,
        Base::Current => server.oplog_frontiers(),
    };

    let merged =
        merge(&server, &base, &scenario.revision, AGENT).map_err(|e| format!("revision: {e}"))?;
    server.import(&merged.update).unwrap();
    sync(&server, &human);

    let out = export(&server).unwrap();
    if out != scenario.expected_markdown {
        return Err(format!(
            "--- expected\n{}--- got\n{out}",
            scenario.expected_markdown
        ));
    }
    if read_tree(&server).unwrap() != read_tree(&human).unwrap() {
        return Err("replicas diverged".into());
    }
    if let Some(ops) = scenario.expected_ops
        && merged.ops != ops
    {
        return Err(format!("expected {ops} ops, got {}", merged.ops));
    }
    Ok(())
}

#[test]
fn every_shared_merge_scenario_holds() {
    let table = load();
    assert!(!table.scenarios.is_empty());
    let failures: Vec<String> = table
        .scenarios
        .iter()
        .filter_map(|scenario| {
            run(scenario)
                .err()
                .map(|why| format!("{}: {why}", scenario.name))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
