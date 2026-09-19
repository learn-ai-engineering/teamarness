use teamarness_core::{CoreRuntime, PublicIdentity, WorkspaceId};

async fn low_level(runtime: &CoreRuntime, workspace: WorkspaceId, peer: PublicIdentity) {
    let _ = runtime.replicate_records_to(workspace, "dev.example.package", peer).await;
}

fn main() {}
