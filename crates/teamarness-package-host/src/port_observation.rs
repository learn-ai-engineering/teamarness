//! Optional, process-local and payload-free Port routing observations.
//! A lagged receiver reports `RecvError::Lagged`; diagnostics never own a routing permit.
use tokio::sync::broadcast;

/// One Host-side route stage. IDs are local to their respective process generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortStage {
    Rejected,
    Dispatched,
    Completed,
    Failed,
    Subscribed,
    Event,
    Closed,
}

/// No request, response, actor, grant, record or Message content is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortObservation {
    pub kind: &'static str,
    pub contract: Option<String>,
    pub port: Option<String>,
    pub consumer_id: String,
    pub consumer_package: String,
    pub consumer_generation: u64,
    pub provider_id: Option<String>,
    pub provider_generation: Option<u64>,
    pub stage: PortStage,
    /// Stable Host code only. A missing Provider terminal is not success.
    pub code: Option<&'static str>,
    pub outcome_uncertain: bool,
}

pub type PortObservations = broadcast::Receiver<PortObservation>;

pub(crate) fn channel() -> broadcast::Sender<PortObservation> {
    // No receiver means no work is queued. Full receivers lag instead of blocking Port work.
    broadcast::channel(128).0
}
