//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! How macOS file events (notify 8.2.1, FSEvents backend) behave for the save patterns that
//! folder mapping must survive. Prints what each pattern produced.
//!
//! Run: cargo run --release -p teamarness-documents-package --example spike_fs_watcher

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

fn drain(rx: &Receiver<notify::Result<Event>>, quiet: Duration) -> Vec<(Instant, Event)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.recv_timeout(quiet) {
        out.push((Instant::now(), event.unwrap()));
    }
    out
}

fn summary(events: &[(Instant, Event)], dir: &Path) -> String {
    let mut kinds = Vec::new();
    for (_, e) in events {
        let names: Vec<String> = e.paths.iter().map(|p| p.strip_prefix(dir).unwrap_or(p).display().to_string()).collect();
        kinds.push(format!("{:?}{names:?}", short(&e.kind)));
    }
    kinds.join(", ")
}

fn short(kind: &EventKind) -> String {
    format!("{kind:?}").replace("Modify(", "M(").replace("Create(", "C(").replace("Remove(", "R(")
}

fn main() {
    let dir = std::env::temp_dir().join("PROTOTYPE-wipe-me-fsdocs-watch");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default()).unwrap();
    watcher.watch(&dir, RecursiveMode::Recursive).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = drain(&rx, Duration::from_millis(200));
    let quiet = Duration::from_millis(600);
    let file = dir.join("plan.md");

    // 1. Plain in-place write: latency to first event.
    let mut latencies = Vec::new();
    fs::write(&file, "v0").unwrap();
    let _ = drain(&rx, quiet);
    for i in 0..10 {
        let t = Instant::now();
        fs::write(&file, format!("v{i}")).unwrap();
        let ev = drain(&rx, quiet);
        if let Some((at, _)) = ev.first() {
            latencies.push(at.duration_since(t).as_secs_f64() * 1e3);
        }
    }
    latencies.sort_by(f64::total_cmp);
    println!("1 in-place write latency ms: min {:.1} median {:.1} max {:.1} ({} of 10 seen)", latencies[0], latencies[latencies.len() / 2], latencies[latencies.len() - 1], latencies.len());

    // 2. Atomic save (VS Code / most editors / our own write_back): temp + rename over.
    let tmp = dir.join(".plan.md.tmp");
    fs::write(&tmp, "atomic").unwrap();
    fs::rename(&tmp, &file).unwrap();
    let ev = drain(&rx, quiet);
    println!("2 atomic save: {}", summary(&ev, &dir));

    // 3. Vim-style backup save: rename original away, write a new file, delete the backup.
    let backup = dir.join("plan.md~");
    fs::rename(&file, &backup).unwrap();
    fs::write(&file, "vim").unwrap();
    fs::remove_file(&backup).unwrap();
    let ev = drain(&rx, quiet);
    println!("3 vim backup save: {}", summary(&ev, &dir));

    // 4. Burst: 100 writes in about one second — how many events, and is the last one seen?
    let t = Instant::now();
    for i in 0..100 {
        fs::write(&file, format!("burst {i}")).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    let ev = drain(&rx, quiet);
    let last = ev.last().map(|(at, _)| at.duration_since(t).as_secs_f64() * 1e3).unwrap_or(0.0);
    println!("4 burst of 100 writes: {} events, last event at {last:.0} ms (writes took ~1000 ms)", ev.len());

    // 5. Bulk change of 300 files at once (git checkout / pull / sed -i across a folder).
    let names: Vec<PathBuf> = (0..300).map(|i| dir.join(format!("doc-{i:03}.md"))).collect();
    for n in &names { fs::write(n, "a").unwrap(); }
    let _ = drain(&rx, quiet);
    let t = Instant::now();
    for n in &names { fs::write(n, "b").unwrap(); }
    let ev = drain(&rx, Duration::from_millis(1500));
    let seen: BTreeSet<&PathBuf> = ev.iter().flat_map(|(_, e)| e.paths.iter()).collect();
    let covered = names.iter().filter(|n| seen.contains(n)).count();
    let span = ev.last().map(|(at, _)| at.duration_since(t).as_secs_f64() * 1e3).unwrap_or(0.0);
    println!("5 bulk 300-file change: {} events, {covered}/300 files named, rescan flags: {}, last at {span:.0} ms", ev.len(), ev.iter().filter(|(_, e)| e.need_rescan()).count());

    // 6. Folder rename and nested folders.
    let sub = dir.join("specs/api");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("auth.md"), "x").unwrap();
    let _ = drain(&rx, quiet);
    fs::rename(dir.join("specs"), dir.join("specifications")).unwrap();
    let ev = drain(&rx, quiet);
    println!("6 folder rename: {}", summary(&ev, &dir));

    // 7. Case-insensitive names: two titles differing only by case.
    fs::write(dir.join("Roadmap.md"), "upper").unwrap();
    fs::write(dir.join("roadmap.md"), "lower").unwrap();
    let files: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.to_lowercase() == "roadmap.md").collect();
    println!("7 'Roadmap.md' then 'roadmap.md': directory has {files:?} containing {:?}", fs::read_to_string(dir.join("Roadmap.md")).unwrap());
    let _ = drain(&rx, quiet);

    // 8. Unicode normalisation: NFC title, NFD name on disk.
    let nfc = "Caf\u{e9}.md";
    fs::write(dir.join(nfc), "x").unwrap();
    let listed: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("Caf")).collect();
    println!("8 NFC name written, listed back as bytes equal to NFC: {}", listed.iter().any(|n| n == nfc));
    let _ = fs::remove_dir_all(&dir);
}
