# 0041 — A Collection can be mapped to a folder of Markdown files on one Installation

**Status:** accepted (2026-11-16)
**Supersedes in part:** none. It removes "folder mapping" from the `TEAM.SYS.DOCS-R10` non-goals.
**Refines:** ADR 0032 (a mapped file is one more Markdown import path, through a Person's revision of a base version); ADR 0042 (the dropped text that 0042 records is refused, not imported, for a mapped file)

## Context

Documents bodies are Loro trees, and Canonical Markdown is their interchange (ADR 0032). A Member who wants to edit a Document in a text editor, or let a command-line agent edit it, has to copy Markdown in and out by hand. The `filesystem-docs` workstream maps a Collection to a folder, so a file and its Document follow each other.

Three facts shape the design:

- Import of outside Markdown can drop text (ADR 0042, `markdown-contract.md`). A first sync that writes the export back would delete that text from the author's file.
- File watchers lose and merge events, and a process can stop at any point between appending a record and saving what it did.
- A folder can sit on a drive that is unmounted, or inside a git working tree whose pulls and checkouts change many files at once.

## Decision

**A Backend module, not a Core feature.** Folder sync is the `folder_sync` child module of the Documents Backend. Core, the Package Host, the Shell, the SDK and the protocol crates do not change. It has five parts:

- a watcher adapter (`watch.rs`, `notify` 8.2.1). Events are only hints: each one marks its mapping, and a 100 ms debounce runs a scan.
- a hash-scan reconciler (`reconcile.rs`). It compares each file's SHA-256 with the state it last saw, and decides what happened.
- a file codec (`codec.rs`): file names, hashes, atomic writes and the splice writer.
- a mapping store (migration `0010_folder_mappings.sql`): mappings, file state and refused files.
- the import path. A changed file becomes the local Person's Markdown revision of the version the file was last written from, merged as any revision is (R05, R11).

**Mappings belong to one Installation.** They are never replicated. A teammate sees only Document changes.

**Mapping rules.** `documents.mapping.set` checks the folder before it stores anything:

- The path is absolute, exists and is a folder.
- It is not inside the Package Data Directory, and it does not equal, contain or sit inside another mapping's root. The Package SDK exposes only the Package Data Directory, so the check guards that folder, not the whole Installation data directory: a Member can still map the Installation's profile folder or another Package's folder. Guarding those needs a Host capability, which this ADR does not add.
- If the folder has no `.md` file to sync, the Collection is exported to it. If the Collection has no Document, the folder is imported into it. If both have content, the mapping is refused (`not_empty`). Merging two non-empty sides is not offered.
- If the folder is inside a git working tree, the mapping is refused (`git_working_tree`) until the Member confirms it. Git changes files in bulk, and each change becomes an edit for the whole team. Teamarness does not detect or deduplicate edits that git delivers.

A dry run returns the pre-mapping report and changes nothing. The report lists the files that will be imported, the files that will be refused with their reasons, the files the first write-back will reformat, and the git root. "Create Collection from folder" checks the folder as an empty Collection first, so a refused folder creates no Collection.

**What is synced.** The stored mapping mode selects flat or recursive scanning. Flat mode includes only regular root files. Recursive mode includes eligible nested files and derives placement from their parent paths (ADR 0043). Both modes exclude hidden paths, temporary files, and extensions other than lowercase `.md`. Folder sync never deliberately follows a symbolic link, writes outside the root, or overwrites an unread source edit. An unavailable or refused subtree protects its tracked Documents from deletion. Existing mappings remain flat until the Member previews and confirms **Enable subfolders** on this Installation. New mappings include subfolders in their ordinary preview. ADR 0043 defines the in-place mode transition, incremental report, and relocation refusals.

**Lossy files are refused.** Before each import, `markdown::lost_tokens` imports and exports the file's text and compares its alphanumeric tokens. Reference labels and ordered-list item numbers are exempt, because a resolved reference and a renumbered list lose them legitimately. If any token is missing, the file is refused as `lossy`, with a sample of the lost tokens, and its bytes stay as they are. A file over the Documents bounds (R09), one that is not UTF-8, and one the schema cannot hold are refused the same way. A refusal records the file's hash, so the same bytes are not tried again until the file changes.

**Crash safety.**

- An import's request ID is derived from the Document, the base version and the file's hash. Before it stages an import, the reconciler looks the ID up. An ID already in the record log advances the base instead of appending again.
- A write-back records the hash and the version it is about to write before it renames the file into place. A file found with that hash adopts that version, and it is not imported.
- A missing or unreadable root marks the mapping `unavailable` and archives nothing. An established mapping with zero eligible readable Markdown files in its active scan scope does the same, including deletion of its only populated subtree. Any eligible readable file prevents this guard, even if not previously tracked. A deleted subtree archives its Documents only when absence is known and eligible files remain elsewhere. Unreadable, symlink-replaced, evicted, and refused prefixes do not mean deletion. An iCloud placeholder protects its corresponding file or subtree, including `.Plans.md.icloud` for a directory. Existing tracked prefixes excluded by the reserved `.teamarness-tmp` suffix stay protected; temporary entries are never imported.
- Write-back keeps the replaced file's permissions, and a case-only rename never replaces another file whose name differs only in case. Whole relative paths use case-folded NFC keys, so equal basenames in different folders are independent. Directory aliases are refused rather than silently merged.
- Migration 0012 stores unfinished relocation intents with source, destination, expected hash, and placement operation identity. Recovery reserves both paths, preserves body bases and pending writes, and reuses the same placement identity. It adopts a completed move or refuses changed or unsafe bytes without creating another Document. Preflight refusals leave no recovery intent and allow later placement or source edits. An attempt cancelled before this invocation attempts movement can also release its intent. Persisted ambiguous intents remain reserved: matching source bytes cannot prove that a move never happened, because the source could have been recreated after a crash. This includes ambiguous intents retained by earlier builds and interruptions before movement. Migration 0011 remains unchanged.
- Remote placement imports source edits before movement, creates safe parents, and checks normalized destination ownership against both tracked paths and actual entries. An occupied or aliased destination refuses only that file. Reserved `.teamarness-tmp` folder segments refuse export, placement, and recursive activation without changing logical metadata. Hash-only local moves preserve identity only with an unambiguous unchanged-hash match. Cleanup removes only empty parents made empty by Teamarness's own successful move or removal, bottom-up, never recursively and never the root.
- The first sync of a file never rewrites it. Write-back splices: unchanged top-level blocks keep their bytes. If the spliced text would not have the export's Canonical Markdown, the export is written instead.

## Rejected alternatives

- **Write the export on first sync.** It rewrites every hand-formatted file, and it deletes the text that import drops.
- **Import lossy files and warn.** The next write-back deletes the lost text from the author's file.
- **Trust watcher events as the source of truth.** Events are lost, merged and reordered on every platform. A scan by hash finds the same state however the events arrive.
- **Replicate mappings to the Workspace.** A path means nothing on another device.
- **Refuse git working trees outright.** Many Members keep notes in a repository. A warning that needs a confirmation states the risk and leaves the choice to them.
- **Merge a non-empty folder into a non-empty Collection.** Pairing files with Documents by name can join unrelated text. It can return when a real need appears.

## Consequences

- A bulk change, such as a git checkout of a mapped folder, becomes one edit per file for the whole team.
- Slice B body imports remain ordinary Person revisions. Slice C placement adds kind 12 and format generation 2 under ADR 0043; it changes no body node.
- The loss check is conservative. An entity such as `&copy;`, which exports as `©`, loses the token `copy`, so its file is refused until it is rewritten.
- An empty Document can be left by a crash between a new file's `document.create` and its first revision. The window is small, and no test injects it.
- Forward migrations 0010–0012 make an upgraded data directory unopenable by older builds. Migration 0011 preserves existing mappings as flat; 0012 adds durable relocation state. ADR 0043 describes mixed-version peer recovery.

## Evidence

- `packages/documents-package/backend/src/folder_sync/` (`mapping.rs`, `watch.rs`, `reconcile.rs`, `codec.rs`, `scan.rs`), `store/mappings.rs`, `migrations/0010_folder_mappings.sql`, `markdown/loss.rs`.
- Tests: `folder_sync::reconcile::tests` (flat and recursive reconciliation, first-sync bytes, loss and bounds refusals, repeated moves, case and Unicode paths, subtree protection, the zero-files guard, archive/restore, cleanup, relocation and body-write crash recovery, no echo, and an offline batch of 50 edits), `folder_sync::mapping::{tests, request_tests}` (each refusal reason, dry run, Create Collection from folder, listing, Unmap), `folder_sync::watch::tests`, and the two-Installation system test `tests/system/tests/documents_folder_sync.rs` (nested concurrent saves, independent legacy consent, repeated moves, folder rename, cleanup, subtree archive/restore, the zero-files guard, and 50 nested offline edits without a UI mount). The settings view is tested in `packages/documents-package/ui/src/settings.test.ts`.
- **Weaknesses.** The watcher is qualified only locally on macOS (FSEvents), through `folder_sync::watch::tests` and the system test. CI runs no `cargo test`, so inotify (Linux) and Windows are not qualified. A save from a real editor is a manual check.

### Provenance (non-versioned)

- `filesystem-docs` workstream: the design (section B) and the slice B plan.
