use teamarness_core::{CoreRuntime, PrincipalId, WorkspaceId};

fn extract_agent_secret(runtime: &CoreRuntime, workspace: WorkspaceId, agent: PrincipalId) {
    let _ = runtime.agent_secret_key(workspace, agent);
}

fn main() {}
