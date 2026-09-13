use std::str::FromStr;

use resonance_core::WorkspaceInvitation;

fn main() {
    let invitation = WorkspaceInvitation::from_str("not-an-invitation").unwrap();
    let _ = invitation.endpoint_ticket();
}
