use serde_json::{Value, json};
use teamarness_package_sdk::{
    Empty, Router,
    ports::PortClient,
    testing::{HarnessBuilder, ScriptedCapability},
};

async fn settings(reply: Option<Value>) -> Result<Value, teamarness_package_sdk::BackendError> {
    let scripted = ScriptedCapability::new().calls(move |method, payload| {
        if let Some(reply) = &reply { return Ok(reply.clone()); }
        match method {
            "available" => {
                assert_eq!(payload, json!({"access":"agent"}));
                Ok(json!({"contracts":[{"contract":"com.example.messages","major":1,"minor":null,"description":null,"available":false,"enabled":true,"enabledMajor":1,"metadataSha256":null}]}))
            }
            "enable" => { assert_eq!(payload, json!({"contract":"com.example.messages","major":1})); Ok(json!({"contract":"com.example.messages","major":1,"enabled":true})) }
            "disable" => { assert_eq!(payload, json!({"contract":"com.example.messages"})); Ok(json!({"contract":"com.example.messages","enabled":false})) }
            _ => panic!("unexpected method {method}"),
        }
    });
    let router = Router::stateless().request("settings.change", |context, _: Empty| async move {
        let client = PortClient::new(
            context.capabilities().clone(),
            context
                .initialization()
                .capability("teamarness.package-ports")
                .unwrap()
                .grant
                .clone(),
        );
        client.enable("com.example.messages", 1).await?;
        let rows = client.agent_contracts().await?;
        client.disable("com.example.messages").await?;
        Ok(json!({"savedMajor":rows[0].enabled_major,"available":rows[0].available}))
    });
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .capability("teamarness.package-ports", scripted)
        .start()
        .await;
    harness.activate().await.unwrap();
    let result = harness.request("settings.change", json!({})).await;
    harness.shutdown().await.unwrap();
    result
}

#[tokio::test]
async fn agent_helpers_use_implicit_scope_and_preserve_unavailable_selections() {
    assert_eq!(
        settings(None).await.unwrap(),
        json!({"savedMajor":1,"available":false})
    );
}

#[tokio::test]
async fn enable_does_not_accept_a_false_or_cross_contract_acknowledgement() {
    for reply in [
        json!({"contract":"com.example.messages","major":1,"enabled":false}),
        json!({"contract":"com.example.other","major":1,"enabled":true}),
        json!({"contract":"com.example.messages","major":2,"enabled":true}),
        json!({"contract":"com.example.messages","major":1,"enabled":true,"extra":true}),
    ] {
        assert_eq!(
            settings(Some(reply)).await.unwrap_err().code(),
            "package.invalid_capability_response"
        );
    }
}
