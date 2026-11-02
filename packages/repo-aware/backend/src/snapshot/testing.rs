//! Scoped rendezvous for cancellation tests. Normal builds exclude this module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use tokio::sync::oneshot;

struct Gate {
    started: Mutex<Option<oneshot::Sender<()>>>,
    interrupted: Mutex<Option<oneshot::Sender<()>>>,
    released: Mutex<bool>,
    wake: Condvar,
}

fn gates() -> &'static Mutex<HashMap<PathBuf, Arc<Gate>>> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn gate_for(path: &str) -> Option<Arc<Gate>> {
    gates().lock().unwrap().get(Path::new(path)).cloned()
}

/// Holds the next snapshot at its first real status item for one repository.
///
/// Dropping the control always releases its worker, including during a test panic.
pub struct SnapshotWalkPause {
    repository: PathBuf,
    gate: Arc<Gate>,
    started: Option<oneshot::Receiver<()>>,
    interrupted: Option<oneshot::Receiver<()>>,
}

impl SnapshotWalkPause {
    /// Installs a rendezvous for a test repository containing a status item.
    #[must_use]
    pub fn new(repository: &Path) -> Self {
        let repository = std::fs::canonicalize(repository).expect("test repository exists");
        let (started, started_rx) = oneshot::channel();
        let (interrupted, interrupted_rx) = oneshot::channel();
        let gate = Arc::new(Gate {
            started: Mutex::new(Some(started)),
            interrupted: Mutex::new(Some(interrupted)),
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        let mut registered = gates().lock().unwrap();
        if registered.contains_key(&repository) {
            drop(registered);
            panic!("only one walk pause may own a test repository");
        }
        registered.insert(repository.clone(), gate.clone());
        Self {
            repository,
            gate,
            started: Some(started_rx),
            interrupted: Some(interrupted_rx),
        }
    }

    /// Observes entry into the real status iterator; callers apply a bounded timeout.
    pub async fn wait_started(&mut self) {
        self.started
            .take()
            .expect("observe walk start once")
            .await
            .expect("walk start sender remains owned");
    }

    /// Lets the blocking worker continue and observe its actual cancellation flag.
    pub fn release(&self) {
        *self.gate.released.lock().unwrap() = true;
        self.gate.wake.notify_all();
    }

    /// Observes this repository's real interrupted return, not another test's counter.
    pub async fn wait_interrupted(&mut self) {
        self.interrupted
            .take()
            .expect("observe interruption once")
            .await
            .expect("interruption sender remains owned");
    }
}

impl Drop for SnapshotWalkPause {
    fn drop(&mut self) {
        self.release();
        let mut registered = gates().lock().unwrap();
        if registered
            .get(&self.repository)
            .is_some_and(|gate| Arc::ptr_eq(gate, &self.gate))
        {
            registered.remove(&self.repository);
        }
    }
}

pub(super) fn checkpoint(path: &str) {
    let Some(gate) = gate_for(path) else {
        return;
    };
    let Some(started) = gate.started.lock().unwrap().take() else {
        return;
    };
    let _ = started.send(());
    let mut released = gate.released.lock().unwrap();
    while !*released {
        released = gate.wake.wait(released).unwrap();
    }
}

pub(super) fn interrupted(path: &str) {
    if let Some(gate) = gate_for(path)
        && let Some(interrupted) = gate.interrupted.lock().unwrap().take()
    {
        let _ = interrupted.send(());
    }
}
