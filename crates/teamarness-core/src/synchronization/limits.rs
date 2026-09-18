//! Every synchronization bound, in one place.
//!
//! A bound that lives here is one number with one meaning, so a change is a
//! change to the whole system rather than to one call site.

/// How far beyond the highest local causal height a received record's order key
/// may reach.
///
/// A Member that has been writing while offline is legitimately ahead, so the
/// window is wide. Beyond it, the height is a claim this Installation cannot
/// check, and the operation is rejected rather than allowed to push every later
/// record to the end of the order.
pub(crate) const MAX_HEIGHT_ADVANCE: u64 = 1 << 20;
