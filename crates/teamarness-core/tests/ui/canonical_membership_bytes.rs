use teamarness_core::MembershipSnapshot;

fn canonical(snapshot: &MembershipSnapshot) {
    let _ = snapshot.canonical_bytes();
}

fn main() {}
