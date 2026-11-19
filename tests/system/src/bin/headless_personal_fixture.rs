#![forbid(unsafe_code)]
//! Unknown Personal Package used only by headless request and administration qualification.
use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use teamarness_package_sdk::capabilities::agent_binding::{AgentBinding, BoundAgent};
use teamarness_package_sdk::{Activation, BackendError, Context, Empty, Router};
use tokio::sync::Mutex;

struct State {
    directory: PathBuf,
    durable: Mutex<Durable>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Durable {
    value: String,
    calls: u64,
    mutations: u64,
    started: u64,
    secret_issued: bool,
    #[serde(default)]
    suspends: u64,
    #[serde(default)]
    recovers: u64,
}

impl State {
    fn save(&self, durable: &Durable) -> Result<(), BackendError> {
        let bytes = serde_json::to_vec(durable).map_err(|_| storage())?;
        let temporary = self.directory.join("state.tmp");
        let file = std::fs::File::create(&temporary).map_err(|_| storage())?;
        use std::io::Write;
        (&file).write_all(&bytes).map_err(|_| storage())?;
        file.sync_all().map_err(|_| storage())?;
        std::fs::rename(temporary, self.directory.join("state.json")).map_err(|_| storage())?;
        std::fs::File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| storage())
    }
    async fn lifecycle(&self, operation: &str) -> Result<(), BackendError> {
        let mut durable = self.durable.lock().await;
        match operation {
            "suspend" => durable.suspends += 1,
            "recover" => durable.recovers += 1,
            _ => unreachable!(),
        }
        self.save(&durable)?;
        self.lose_reply_if_armed(operation)
    }
    fn lose_reply_if_armed(&self, operation: &str) -> Result<(), BackendError> {
        let fault = self.directory.join("lose-lifecycle-reply");
        match std::fs::read_to_string(&fault) {
            Ok(armed) if armed == operation => {
                std::fs::remove_file(fault).map_err(|_| storage())?;
                // The durable Package-owned effect precedes loss of the terminal protocol reply.
                std::process::exit(73);
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(storage()),
        }
    }
    async fn called(&self) -> Result<(), BackendError> {
        let mut durable = self.durable.lock().await;
        durable.calls += 1;
        self.save(&durable)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation: Activation| async move {
        let directory = activation.data_directory();
        let count = directory.join("activations");
        let prior = match std::fs::read_to_string(&count) {
            Ok(text) => text.parse::<u64>().map_err(|_| storage())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return Err(storage()),
        };
        std::fs::write(count, (prior + 1).to_string()).map_err(|_| storage())?;
        let retained = directory.join("retained");
        if !retained.exists() {
            std::fs::write(retained, b"fixture-owned persistent value").map_err(|_| storage())?;
        }
        let durable = match std::fs::read(directory.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| storage())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Durable::default(),
            Err(_) => return Err(storage()),
        };
        let state = State { directory: directory.to_owned(), durable: Mutex::new(durable) };
        // Restart qualification uses the existing durable activation count before dropping reply.
        state.lose_reply_if_armed("restart")?;
        Ok(state)
    })
    .on_suspend(|state| async move { state.lifecycle("suspend").await })
    .on_resume(|state| async move { state.lifecycle("recover").await })
    .request("proof.settings.get", |context: Context<State>, _: Empty| async move {
        context.state().called().await?;
        Ok(json!({"value":context.state().durable.lock().await.value}))
    })
    .request("proof.settings.set", |context: Context<State>, payload: Value| async move {
        context.state().called().await?;
        let mut durable = context.state().durable.lock().await;
        durable.value = payload["value"].as_str().ok_or_else(storage)?.into();
        context.state().save(&durable)?;
        Ok(json!({"value":durable.value}))
    })
    .request("proof.status", |context: Context<State>, _: Empty| async move {
        // Status itself does not increment calls; rejected inputs can be observed independently.
        let durable = context.state().durable.lock().await;
        Ok(json!({"calls":durable.calls,"mutations":durable.mutations,"started":durable.started}))
    })
    .request("proof.secret", |context: Context<State>, _: Empty| async move {
        context.state().called().await?;
        let mut durable = context.state().durable.lock().await;
        let secret = (!durable.secret_issued).then_some("ONE_TIME_SECRET_CANARY");
        durable.secret_issued = true;
        context.state().save(&durable)?;
        Ok(json!({"secret":secret}))
    })
    .request("proof.error", |context: Context<State>, _: Empty| async move {
        context.state().called().await?;
        Err::<Value, _>(BackendError::new("proof.deliberate", false, json!({"reason":"expected"})))
    })
    .request("proof.invalid", |context: Context<State>, _: Empty| async move {
        context.state().called().await?;
        Ok(json!({"wrong":true}))
    })
    .request("proof.mutate", |context: Context<State>, payload: Value| async move {
        context.state().called().await?;
        {
            let mut durable = context.state().durable.lock().await;
            durable.started += 1;
            context.state().save(&durable)?;
        }
        // simulated-delay: a bounded request deliberately outlives its local observer.
        tokio::time::sleep(std::time::Duration::from_millis(payload["delayMs"].as_u64().ok_or_else(storage)?)).await;
        let mut durable = context.state().durable.lock().await;
        durable.mutations += 1;
        context.state().save(&durable)?;
        Ok(json!({"mutations":durable.mutations}))
    })
    .request("proof.binding.current", |context: Context<State>, _: Empty| async move {
        let agent = binding(&context)?.current().await?;
        Ok(json!({"agent":agent.map(agent_json)}))
    })
    .request("proof.binding.set", |context: Context<State>, payload: Value| async move {
        let agent = binding(&context)?.bind(payload["principalId"].as_str().ok_or_else(storage)?).await?;
        Ok(json!({"agent":agent_json(agent)}))
    })
    .request("proof.binding.clear", |context: Context<State>, _: Empty| async move {
        binding(&context)?.unbind().await?;
        Ok(json!({"agent":null}))
    })
    .run().await
}

fn binding(context: &Context<State>) -> Result<AgentBinding, BackendError> {
    AgentBinding::from_initialization(context.initialization(), context.capabilities().clone())
}
fn agent_json(agent: BoundAgent) -> Value {
    json!({"principalId":agent.principal_id,"identity":agent.identity,"name":agent.name})
}
fn storage() -> BackendError {
    BackendError::new("qualification.storage", false, json!({}))
}
