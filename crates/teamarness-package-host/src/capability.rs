use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value, json};
use teamarness_core::{
    CoreRuntime, MembershipEpoch, MembershipRole, PublicIdentity, PublicSignature, SigningContext,
};
use tokio::sync::{Mutex, Semaphore, mpsc, watch};

use crate::protocol::{CapabilityFailure, HostStreamSender};
use crate::{InstallationPublicIdentity, PackageErrorCode, PackageIdentity, WorkspaceId};

const SIGNING_CONTEXT: &str = "package-signing/v1";
const MAX_PURPOSE_BYTES: usize = 64;
const MAX_SIGNING_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_STREAM_CREDIT: u64 = 32;

/// The Host-wide bound on concurrent blocking Core work for Package capability
/// calls.
///
/// Core's state lock is synchronous, so a capability call does its Core work on
/// a blocking thread. Without a bound, Packages could occupy the whole blocking
/// pool and the call deadline would stop meaning anything.
#[derive(Clone)]
pub(crate) struct CoreCallLimiter {
    permits: Arc<Semaphore>,
}

impl CoreCallLimiter {
    pub(crate) const PERMITS: usize = 8;

    pub(crate) fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(Self::PERMITS)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_permits(permits: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits)),
        }
    }

    /// Waits for a permit, then runs `f` on a blocking thread, holding the
    /// permit until `f` returns.
    ///
    /// The permit moves into the closure, so a call that outlived its deadline
    /// still releases its permit only when its Core work really ends. `started`
    /// becomes true as the closure's first statement, which is what makes an
    /// `append` outcome uncertain rather than failed.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        started: Arc<AtomicBool>,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PackageErrorCode> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
        tokio::task::spawn_blocking(move || {
            started.store(true, Ordering::Release);
            let value = f();
            drop(permit);
            value
        })
        .await
        .map_err(|_| PackageErrorCode::CapabilityUnavailable)
    }
}

#[derive(Clone)]
struct Descriptor {
    identity: &'static str,
    major: u16,
    minor: u16,
    kind: CapabilityKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapabilityKind {
    WorkspaceContext,
    Membership,
    Signing,
    ImmutableRecordLog,
    #[cfg(test)]
    Neutral,
}

#[derive(Clone)]
pub(crate) struct CapabilityRequirement {
    pub(crate) identity: String,
    pub(crate) major: u16,
    pub(crate) min_minor: u16,
    pub(crate) max_minor: u16,
}

#[derive(Clone)]
pub(crate) struct CapabilityScope {
    pub(crate) package_identity: PackageIdentity,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) workspace_name: String,
    pub(crate) installation: InstallationPublicIdentity,
}

#[derive(Clone)]
struct BoundGrant {
    descriptor: Descriptor,
    package_identity: PackageIdentity,
    workspace_id: WorkspaceId,
    installation: InstallationPublicIdentity,
    generation: u64,
}

#[derive(Clone)]
pub(crate) struct AdvertisedGrant {
    pub(crate) grant: String,
    pub(crate) identity: &'static str,
    pub(crate) major: u16,
    pub(crate) minor: u16,
}

pub(crate) struct GenerationCapabilities {
    core: Arc<CoreRuntime>,
    limiter: CoreCallLimiter,
    scope: CapabilityScope,
    generation: u64,
    grants: BTreeMap<String, BoundGrant>,
    advertised: Vec<AdvertisedGrant>,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    cancelled: watch::Sender<bool>,
}

impl GenerationCapabilities {
    pub(crate) fn bind(
        core: Arc<CoreRuntime>,
        scope: CapabilityScope,
        requirements: &[CapabilityRequirement],
        generation: u64,
        limiter: CoreCallLimiter,
    ) -> Result<Arc<Self>, PackageErrorCode> {
        let registry = Registry::production();
        let mut grants = BTreeMap::new();
        let mut advertised = Vec::with_capacity(requirements.len());
        for requirement in requirements {
            let descriptor = registry.negotiate(requirement)?;
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
            let grant = URL_SAFE_NO_PAD.encode(random);
            grants.insert(
                grant.clone(),
                BoundGrant {
                    descriptor: descriptor.clone(),
                    package_identity: scope.package_identity.clone(),
                    workspace_id: scope.workspace_id.clone(),
                    installation: scope.installation.clone(),
                    generation,
                },
            );
            advertised.push(AdvertisedGrant {
                grant,
                identity: descriptor.identity,
                major: descriptor.major,
                minor: descriptor.minor,
            });
        }
        let (cancelled, _) = watch::channel(false);
        Ok(Arc::new(Self {
            core,
            limiter,
            scope,
            generation,
            grants,
            advertised,
            streams: Arc::new(Mutex::new(BTreeMap::new())),
            cancelled,
        }))
    }

    pub(crate) fn advertised(&self) -> &[AdvertisedGrant] {
        &self.advertised
    }

    /// Validates the request on the asynchronous task, then runs the Core work
    /// on a blocking thread through the shared limiter.
    pub(crate) async fn call(
        &self,
        grant: Option<&str>,
        payload: &Value,
        started: Arc<AtomicBool>,
    ) -> Result<Value, CapabilityFailure> {
        self.call_inner(grant, payload, started)
            .await
            .map_err(CapabilityFailure::certain)
    }

    async fn call_inner(
        &self,
        grant: Option<&str>,
        payload: &Value,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let grant = self.bound_grant(grant)?;
        let object = payload
            .as_object()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let method = string_field(object, "method")?;
        match grant.descriptor.kind {
            CapabilityKind::WorkspaceContext => {
                self.workspace_context(method, object, started).await
            }
            CapabilityKind::Membership => self.membership(method, object, started).await,
            CapabilityKind::Signing => self.signing(method, object, started).await,
            CapabilityKind::ImmutableRecordLog => {
                self.immutable_record(method, object, started).await
            }
            #[cfg(test)]
            CapabilityKind::Neutral => Ok(json!({"neutral":true})),
        }
    }

    pub(crate) async fn open_stream(
        &self,
        stream_id: String,
        topic: &str,
        initial_credit: u64,
        payload: &Value,
        output: HostStreamSender,
    ) -> Result<(), PackageErrorCode> {
        if topic != "capability.stream" || initial_credit != MAX_STREAM_CREDIT {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let object = payload
            .as_object()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let grant = self.bound_grant(Some(string_field(object, "grant")?))?;
        let method = string_field(object, "method")?;
        let core_workspace =
            teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let after_epoch =
            if grant.descriptor.kind == CapabilityKind::Membership && method == "subscribe" {
                exact_fields(object, &["grant", "method", "afterEpoch"])?;
                Some(decimal_field(object, "afterEpoch")?)
            } else {
                None
            };
        let after_cursor = if grant.descriptor.kind == CapabilityKind::ImmutableRecordLog
            && method == "subscribe"
        {
            exact_fields(object, &["grant", "method", "afterCursor"])?;
            Some(optional_cursor(object.get("afterCursor"))?)
        } else {
            None
        };
        if after_epoch.is_none() && after_cursor.is_none() {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let (commands, command_receiver) = mpsc::channel(8);
        let mut streams = self.streams.lock().await;
        if streams.len() >= 32 || streams.insert(stream_id.clone(), commands).is_some() {
            return Err(PackageErrorCode::Overloaded);
        }
        drop(streams);
        let core = self.core.clone();
        let all_streams = self.streams.clone();
        match grant.descriptor.kind {
            CapabilityKind::Membership => {
                let after_epoch = after_epoch.expect("validated membership cursor");
                tokio::spawn(run_membership_stream(
                    core,
                    core_workspace,
                    stream_id,
                    after_epoch,
                    initial_credit,
                    command_receiver,
                    output,
                    all_streams,
                    self.cancelled.subscribe(),
                ));
            }
            CapabilityKind::ImmutableRecordLog => {
                let after_cursor = after_cursor.expect("validated record cursor");
                tokio::spawn(run_record_stream(
                    core,
                    core_workspace,
                    self.scope.package_identity.as_str().to_owned(),
                    stream_id,
                    after_cursor,
                    initial_credit,
                    command_receiver,
                    output,
                    all_streams,
                    self.cancelled.subscribe(),
                ));
            }
            _ => {
                self.streams.lock().await.remove(&stream_id);
                return Err(PackageErrorCode::CapabilityRequestInvalid);
            }
        }
        Ok(())
    }

    pub(crate) async fn add_credit(
        &self,
        stream_id: &str,
        amount: u64,
    ) -> Result<(), PackageErrorCode> {
        if amount == 0 || amount > MAX_STREAM_CREDIT {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let sender = self
            .streams
            .lock()
            .await
            .get(stream_id)
            .cloned()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        sender
            .try_send(StreamCommand::Credit(amount))
            .map_err(|_| PackageErrorCode::Overloaded)
    }

    pub(crate) async fn close_stream(&self, stream_id: &str) {
        if let Some(sender) = self.streams.lock().await.remove(stream_id) {
            let _ = sender.try_send(StreamCommand::Close);
        }
    }

    fn bound_grant(&self, grant: Option<&str>) -> Result<&BoundGrant, PackageErrorCode> {
        let grant = grant
            .and_then(|grant| self.grants.get(grant))
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        if grant.package_identity != self.scope.package_identity
            || grant.workspace_id != self.scope.workspace_id
            || grant.installation != self.scope.installation
            || grant.generation != self.generation
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        Ok(grant)
    }

    async fn workspace_context(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        if method != "get" {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        exact_fields(object, &["method"])?;
        let core = Arc::clone(&self.core);
        let workspace_id = self.scope.workspace_id.as_str().to_owned();
        let workspace_name = self.scope.workspace_name.clone();
        let installation = self.scope.installation.as_str().to_owned();
        self.limiter
            .run(started, move || {
                Ok(json!({
                    "workspaceId": workspace_id,
                    "workspaceName": workspace_name,
                    "installationIdentity": installation,
                    "installationDisplayName": core.display_name(),
                }))
            })
            .await?
    }

    async fn membership(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let core = Arc::clone(&self.core);
        match method {
            "current" => {
                exact_fields(object, &["method"])?;
                self.limiter
                    .run(started, move || {
                        let snapshot = core
                            .membership_snapshot(workspace)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let members = snapshot
                            .members()
                            .iter()
                            .map(|member| {
                                json!({
                                    "identity":member.identity().to_string(),
                                    "role":match member.role() {
                                        MembershipRole::Member => "member",
                                        MembershipRole::Manager => "manager",
                                    }
                                })
                            })
                            .collect::<Vec<_>>();
                        Ok(json!({
                            "workspaceId":snapshot.workspace_id().to_string(),
                            "epoch":snapshot.epoch().value().to_string(),
                            "membershipVersion":snapshot.version().to_string(),
                            "members":members,
                        }))
                    })
                    .await?
            }
            "was_member" => {
                exact_fields(object, &["method", "identity", "atUnixMs"])?;
                let identity = PublicIdentity::from_str(string_field(object, "identity")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let at_unix_ms = signed_decimal_field(object, "atUnixMs")?;
                self.limiter
                    .run(started, move || {
                        let was_member = core
                            .was_member(workspace, identity, at_unix_ms)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        Ok(json!({
                            "wasMember":was_member,
                            "warning":"informational_not_authorization",
                        }))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    async fn immutable_record(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let package = self.scope.package_identity.as_str().to_owned();
        let core = Arc::clone(&self.core);
        match method {
            "append" => {
                exact_fields(object, &["method", "purpose", "canonical"])?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?.to_owned();
                let canonical = decode_bounded(string_field(object, "canonical")?, 262_144)?;
                self.limiter
                    .run(started, move || {
                        let record = core
                            .append_record(workspace, &package, &purpose, &canonical)
                            .map_err(sync_error)?;
                        Ok(json!({
                            "recordId": record.record_id().to_string(),
                            "orderKey": record.order_key().to_string(),
                            "author": record.author().to_string(),
                            "signature": encode_signature(&record.signature())?,
                            "sealedRecord": encode_base64(record.sealed_record()),
                            "durability": record.durability(),
                        }))
                    })
                    .await?
            }
            "open" => {
                exact_fields(object, &["method", "sealedRecords"])?;
                let values = object
                    .get("sealedRecords")
                    .and_then(Value::as_array)
                    .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                if !(1..=32).contains(&values.len()) {
                    return Err(PackageErrorCode::CapabilityRequestInvalid);
                }
                let mut sealed = Vec::with_capacity(values.len());
                let mut total = 0_usize;
                for value in values {
                    let bytes = decode_bounded(
                        value
                            .as_str()
                            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?,
                        512 * 1024,
                    )?;
                    total = total
                        .checked_add(bytes.len())
                        .filter(|total| *total <= 512 * 1024)
                        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                    sealed.push(bytes);
                }
                // Every envelope of one request is opened in one blocking
                // closure, but still one `open_records` call each, so each
                // result is classified independently.
                self.limiter
                    .run(started, move || {
                        let mut results = Vec::with_capacity(sealed.len());
                        for envelope in &sealed {
                            match core.open_records(
                                workspace,
                                &package,
                                std::slice::from_ref(envelope),
                            ) {
                                Ok(records) => {
                                    let record = records
                                        .into_iter()
                                        .next()
                                        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                                    results.push(json!({
                                        "status":"ready",
                                        "recordId":record.record_id().to_string(),
                                        "orderKey":record.order_key().to_string(),
                                        "author":record.author().to_string(),
                                        "purpose":record.purpose(),
                                        "signature":encode_signature(&record.signature())?,
                                        "canonical":encode_base64(record.canonical()),
                                    }));
                                }
                                Err(error) => {
                                    let status = match error {
                                        teamarness_core::SynchronizationError::WrongScope => {
                                            "wrong_scope"
                                        }
                                        teamarness_core::SynchronizationError::CorruptRecord => {
                                            "corrupt"
                                        }
                                        _ => "waiting_for_history_access",
                                    };
                                    results.push(json!({"status":status}));
                                }
                            }
                        }
                        Ok(json!({"results":results}))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    async fn signing(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let core = Arc::clone(&self.core);
        match method {
            "sign" => {
                exact_fields(object, &["method", "purpose", "payload"])?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?;
                let payload = decoded_payload(string_field(object, "payload")?)?;
                let envelope = signing_envelope(
                    &self.scope.package_identity,
                    &self.scope.workspace_id,
                    purpose,
                    &payload,
                )?;
                self.limiter
                    .run(started, move || {
                        let context = SigningContext::new(SIGNING_CONTEXT)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let signature = core
                            .sign(&context, &envelope)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let signature_bytes = decode_hex::<64>(&signature.to_string())?;
                        Ok(json!({
                            "signer":core.public_identity().to_string(),
                            "envelopeVersion":1,
                            "signature":URL_SAFE_NO_PAD.encode(signature_bytes),
                        }))
                    })
                    .await?
            }
            "verify" => {
                exact_fields(
                    object,
                    &["method", "signer", "purpose", "payload", "signature"],
                )?;
                let signer = PublicIdentity::from_str(string_field(object, "signer")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?;
                let payload = decoded_payload(string_field(object, "payload")?)?;
                let signature = URL_SAFE_NO_PAD
                    .decode(string_field(object, "signature")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                if signature.len() != 64 {
                    return Err(PackageErrorCode::CapabilityRequestInvalid);
                }
                let signature = PublicSignature::from_str(&encode_hex(&signature))
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let envelope = signing_envelope(
                    &self.scope.package_identity,
                    &self.scope.workspace_id,
                    purpose,
                    &payload,
                )?;
                self.limiter
                    .run(started, move || {
                        let context = SigningContext::new(SIGNING_CONTEXT)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        Ok(json!({
                            "valid":signer.verify(&context, &envelope, &signature).is_ok()
                        }))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }
}

impl Drop for GenerationCapabilities {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
    }
}

impl crate::protocol::CapabilityHandler for GenerationCapabilities {
    fn call<'a>(
        &'a self,
        grant: Option<&'a str>,
        payload: &'a Value,
        started: Arc<AtomicBool>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    > {
        Box::pin(GenerationCapabilities::call(self, grant, payload, started))
    }

    fn open_stream<'a>(
        &'a self,
        stream_id: String,
        topic: &'a str,
        initial_credit: u64,
        payload: &'a Value,
        output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(GenerationCapabilities::open_stream(
            self,
            stream_id,
            topic,
            initial_credit,
            payload,
            output,
        ))
    }

    fn add_credit<'a>(
        &'a self,
        stream_id: &'a str,
        amount: u64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(GenerationCapabilities::add_credit(self, stream_id, amount))
    }

    fn close_stream<'a>(
        &'a self,
        stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(GenerationCapabilities::close_stream(self, stream_id))
    }
}

struct Registry {
    descriptors: BTreeMap<&'static str, Descriptor>,
}

impl Registry {
    fn production() -> Self {
        let mut registry = Self {
            descriptors: BTreeMap::new(),
        };
        registry.register(Descriptor {
            identity: "teamarness.workspace-context",
            major: 1,
            minor: 0,
            kind: CapabilityKind::WorkspaceContext,
        });
        registry.register(Descriptor {
            identity: "teamarness.membership",
            major: 1,
            minor: 0,
            kind: CapabilityKind::Membership,
        });
        registry.register(Descriptor {
            identity: "teamarness.signing",
            major: 1,
            minor: 0,
            kind: CapabilityKind::Signing,
        });
        registry.register(Descriptor {
            identity: "teamarness.synchronization.immutable-record-log",
            major: 1,
            minor: 0,
            kind: CapabilityKind::ImmutableRecordLog,
        });
        registry
    }

    fn register(&mut self, descriptor: Descriptor) {
        self.descriptors.insert(descriptor.identity, descriptor);
    }

    fn negotiate(
        &self,
        requirement: &CapabilityRequirement,
    ) -> Result<Descriptor, PackageErrorCode> {
        let descriptor = self
            .descriptors
            .get(requirement.identity.as_str())
            .ok_or(PackageErrorCode::CapabilityUnavailable)?;
        if descriptor.major != requirement.major
            || !(requirement.min_minor..=requirement.max_minor).contains(&descriptor.minor)
        {
            return Err(PackageErrorCode::CapabilityVersionUnsupported);
        }
        Ok(descriptor.clone())
    }
}

#[derive(Clone, Copy)]
enum StreamCommand {
    Credit(u64),
    Close,
}

#[allow(clippy::too_many_arguments)]
async fn run_membership_stream(
    core: Arc<CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    stream_id: String,
    mut after_epoch: u64,
    mut credit: u64,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let mut sequence = 0_u64;
    loop {
        if *cancelled.borrow() {
            break;
        }
        while let Ok(command) = commands.try_recv() {
            match command {
                StreamCommand::Credit(amount) => {
                    credit = credit.saturating_add(amount).min(MAX_STREAM_CREDIT)
                }
                StreamCommand::Close => {
                    streams.lock().await.remove(&stream_id);
                    return;
                }
            }
        }
        while credit == 0 {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(StreamCommand::Credit(amount)) => {
                        credit = credit.saturating_add(amount).min(MAX_STREAM_CREDIT)
                    }
                    Some(StreamCommand::Close) | None => {
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                },
                result = cancelled.changed() => {
                    if result.is_err() || *cancelled.borrow() {
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                }
            }
        }
        let core_for_next = core.clone();
        let next = tokio::task::spawn_blocking(move || {
            let mut subscription = core_for_next
                .subscribe_membership_bounded(workspace, MembershipEpoch::new(after_epoch), 1)
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
            subscription
                .next_transition_timeout(std::time::Duration::from_millis(50))
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
        })
        .await;
        let transition = match next {
            Ok(Ok(Some(transition))) => transition,
            Ok(Ok(None)) => continue,
            _ => break,
        };
        sequence = sequence.saturating_add(1);
        let payload = json!({
            "epoch":transition.epoch().value().to_string(),
            "membershipVersion":transition.version().to_string(),
            "cause":transition.cause().to_string(),
            "snapshotDigest":transition.snapshot_digest().to_string(),
        });
        if output.event(&stream_id, sequence, payload).await.is_err() {
            break;
        }
        after_epoch = transition.epoch().value();
        credit -= 1;
        while let Ok(command) = commands.try_recv() {
            match command {
                StreamCommand::Credit(amount) => {
                    credit = credit.saturating_add(amount).min(MAX_STREAM_CREDIT)
                }
                StreamCommand::Close => {
                    streams.lock().await.remove(&stream_id);
                    return;
                }
            }
        }
    }
    streams.lock().await.remove(&stream_id);
    let _ = output.close(&stream_id, "capability_unavailable").await;
}

fn sync_error(error: teamarness_core::SynchronizationError) -> PackageErrorCode {
    match error {
        teamarness_core::SynchronizationError::NotCurrentMember
        | teamarness_core::SynchronizationError::RevokedMember
        // The Package asked for something this Installation cannot serve yet,
        // not something malformed: the scope's content key has not arrived.
        | teamarness_core::SynchronizationError::ContentKeyUnavailable
        | teamarness_core::SynchronizationError::KeyOriginRejected => {
            PackageErrorCode::CapabilityUnavailable
        }
        teamarness_core::SynchronizationError::WorkspaceNotFound
        | teamarness_core::SynchronizationError::InvalidInput
        | teamarness_core::SynchronizationError::WrongScope
        | teamarness_core::SynchronizationError::CorruptRecord
        | teamarness_core::SynchronizationError::CursorScopeMismatch
        | teamarness_core::SynchronizationError::CursorAhead => {
            PackageErrorCode::CapabilityRequestInvalid
        }
        teamarness_core::SynchronizationError::RuntimeUnavailable
        | teamarness_core::SynchronizationError::Storage(_) => {
            PackageErrorCode::CapabilityUnavailable
        }
        _ => PackageErrorCode::CapabilityRequestInvalid,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn encode_signature(
    signature: &teamarness_core::PublicSignature,
) -> Result<String, PackageErrorCode> {
    Ok(encode_base64(&decode_hex::<64>(&signature.to_string())?))
}

fn decode_bounded(value: &str, max: usize) -> Result<Vec<u8>, PackageErrorCode> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(bytes)
}

fn optional_cursor(
    value: Option<&Value>,
) -> Result<Option<teamarness_core::Cursor>, PackageErrorCode> {
    match value {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(value)) => value
            .parse()
            .map(Some)
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid),
        _ => Err(PackageErrorCode::CapabilityRequestInvalid),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_record_stream(
    core: Arc<CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    package: String,
    stream_id: String,
    after: Option<teamarness_core::Cursor>,
    mut credit: u64,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let mut sequence = 0_u64;
    let mut subscription = None;
    loop {
        if *cancelled.borrow() {
            break;
        }
        while let Ok(command) = commands.try_recv() {
            match command {
                StreamCommand::Credit(amount) => {
                    credit = credit.saturating_add(amount).min(MAX_STREAM_CREDIT)
                }
                StreamCommand::Close => {
                    streams.lock().await.remove(&stream_id);
                    return;
                }
            }
        }
        while credit == 0 {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(StreamCommand::Credit(amount)) => {
                        credit = credit.saturating_add(amount).min(MAX_STREAM_CREDIT)
                    }
                    Some(StreamCommand::Close) | None => {
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                },
                result = cancelled.changed() => {
                    if result.is_err() || *cancelled.borrow() {
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                }
            }
        }
        if subscription.is_none() {
            let core_for_subscribe = core.clone();
            let package_for_subscribe = package.clone();
            let next = tokio::task::spawn_blocking(move || {
                core_for_subscribe.subscribe_records(workspace, &package_for_subscribe, after)
            })
            .await;
            match next {
                Ok(Ok(value)) => subscription = Some(value),
                Ok(Err(teamarness_core::SynchronizationError::WorkspaceNotFound)) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Ok(Err(error)) => {
                    eprintln!("record stream subscribe failed: stream={stream_id} error={error}");
                    break;
                }
                Err(error) => {
                    eprintln!(
                        "record stream subscribe task failed: stream={stream_id} error={error}"
                    );
                    break;
                }
            }
        }

        let mut current = subscription.take().expect("record subscription is present");
        let next = tokio::task::spawn_blocking(move || {
            let event = current.next_event_timeout(std::time::Duration::from_millis(50));
            (current, event)
        })
        .await;
        let event = match next {
            Ok((current, Ok(Some(event)))) => {
                subscription = Some(current);
                event
            }
            Ok((current, Ok(None))) => {
                subscription = Some(current);
                continue;
            }
            Ok((_current, Err(_))) => break,
            Err(_) => break,
        };
        sequence = sequence.saturating_add(1);
        let signature = match encode_signature(&event.record().signature()) {
            Ok(signature) => signature,
            Err(_) => break,
        };
        let payload = json!({
            "kind":"record",
            "cursor":event.cursor().to_string(),
            "recordId":event.record_id().to_string(),
            "orderKey":event.order_key().to_string(),
            "author":event.author().to_string(),
            "purpose":event.purpose(),
            "signature":signature,
            "canonical":encode_base64(event.canonical()),
            "sealedRecord":encode_base64(event.sealed_record()),
        });
        if output.event(&stream_id, sequence, payload).await.is_err() {
            eprintln!("record stream delivery failed: stream={stream_id} sequence={sequence}");
            break;
        }
        credit -= 1;
    }
    streams.lock().await.remove(&stream_id);
    let _ = output.close(&stream_id, "capability_unavailable").await;
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), PackageErrorCode> {
    if object.len() == expected.len() && object.keys().all(|key| expected.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(PackageErrorCode::CapabilityRequestInvalid)
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, PackageErrorCode> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)
}

fn decimal_field(object: &Map<String, Value>, name: &str) -> Result<u64, PackageErrorCode> {
    let value = string_field(object, name)?;
    if value != "0" && value.starts_with('0') {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    value
        .parse()
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
}

fn signed_decimal_field(object: &Map<String, Value>, name: &str) -> Result<i64, PackageErrorCode> {
    let value = string_field(object, name)?;
    if value.starts_with('+') || value == "-0" || (value.starts_with('0') && value.len() > 1) {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    value
        .parse()
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
}

fn validated_purpose(value: &str) -> Result<&str, PackageErrorCode> {
    if value.is_empty()
        || value.len() > MAX_PURPOSE_BYTES
        || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(value)
}

fn decoded_payload(value: &str) -> Result<Vec<u8>, PackageErrorCode> {
    let payload = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    if payload.len() > MAX_SIGNING_PAYLOAD_BYTES {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(payload)
}

fn signing_envelope(
    package: &PackageIdentity,
    workspace: &WorkspaceId,
    purpose: &str,
    payload: &[u8],
) -> Result<Vec<u8>, PackageErrorCode> {
    let package_length = u16::try_from(package.as_str().len())
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let workspace_length = u16::try_from(workspace.as_str().len())
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let purpose_length =
        u16::try_from(purpose.len()).map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let mut envelope = Vec::with_capacity(
        2 + 2
            + 2
            + package.as_str().len()
            + 2
            + workspace.as_str().len()
            + 2
            + purpose.len()
            + 4
            + payload.len(),
    );
    envelope.extend_from_slice(&1_u16.to_be_bytes());
    envelope.extend_from_slice(&0_u16.to_be_bytes());
    envelope.extend_from_slice(&package_length.to_be_bytes());
    envelope.extend_from_slice(package.as_str().as_bytes());
    envelope.extend_from_slice(&workspace_length.to_be_bytes());
    envelope.extend_from_slice(workspace.as_str().as_bytes());
    envelope.extend_from_slice(&purpose_length.to_be_bytes());
    envelope.extend_from_slice(purpose.as_bytes());
    envelope.extend_from_slice(&payload_length.to_be_bytes());
    envelope.extend_from_slice(payload);
    Ok(envelope)
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], PackageErrorCode> {
    if value.len() != N * 2 {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    let mut bytes = [0_u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    }
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

    /// Binds the immutable-record capability against a real Core runtime with
    /// one Workspace, and returns the generation and its grant.
    async fn bound_immutable_record(
        core: &Arc<CoreRuntime>,
        limiter: CoreCallLimiter,
    ) -> (Arc<GenerationCapabilities>, String) {
        let workspace = core.create_workspace("Capability limiter").unwrap();
        let capabilities = GenerationCapabilities::bind(
            Arc::clone(core),
            CapabilityScope {
                package_identity: "dev.teamarness.fixture".parse().unwrap(),
                workspace_id: crate::WorkspaceId::new(workspace.id().to_string()).unwrap(),
                workspace_name: workspace.name().to_owned(),
                installation: InstallationPublicIdentity::new(core.public_identity().to_string())
                    .unwrap(),
            },
            &[CapabilityRequirement {
                identity: "teamarness.synchronization.immutable-record-log".into(),
                major: 1,
                min_minor: 0,
                max_minor: 0,
            }],
            1,
            limiter,
        )
        .unwrap();
        let grant = capabilities.advertised()[0].grant.clone();
        (capabilities, grant)
    }

    fn append_request() -> Value {
        json!({"method":"append","purpose":"message.post","canonical":"aGVsbG8"})
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread")]
    async fn capability_call_does_not_block_the_executor_while_core_is_locked() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(
            CoreRuntime::open(CoreRuntimeConfig::new(directory.path().join("core")))
                .await
                .unwrap(),
        );
        let (capabilities, grant) = bound_immutable_record(&core, CoreCallLimiter::new()).await;

        // Core is busy: the append cannot finish until the lock is released.
        let hold = core.hold_state_for_tests();
        let calling = Arc::clone(&capabilities);
        let call = tokio::spawn(async move {
            calling
                .call(
                    Some(&grant),
                    &append_request(),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        });
        let (finished, sibling) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let _ = finished.send(());
        });
        let progressed = tokio::time::timeout(Duration::from_secs(5), sibling)
            .await
            .is_ok_and(|delivered| delivered.is_ok());
        hold.release();
        assert!(
            progressed,
            "the capability call held the executor while Core was locked"
        );
        let appended = call.await.unwrap().expect("the append completes");
        assert_eq!(
            appended.get("durability").and_then(Value::as_str),
            Some("committed")
        );
        drop(capabilities);
        core.shutdown().await.unwrap();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn core_call_limiter_bounds_concurrent_blocking_work() {
        let limiter = CoreCallLimiter::with_permits(2);
        let started = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((std::sync::Mutex::new(false), Condvar::new()));
        let mut running = Vec::new();
        for _ in 0..3 {
            let limiter = limiter.clone();
            let started = Arc::clone(&started);
            let gate = Arc::clone(&gate);
            running.push(tokio::spawn(async move {
                limiter
                    .run(Arc::new(AtomicBool::new(false)), move || {
                        started.fetch_add(1, Ordering::AcqRel);
                        let (open, waiting) = &*gate;
                        let mut open = open.lock().unwrap();
                        while !*open {
                            open = waiting.wait(open).unwrap();
                        }
                    })
                    .await
            }));
        }
        // Two permits, so exactly two closures can be inside the pool; the
        // third cannot start until one of them returns, which needs the gate.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while started.load(Ordering::Acquire) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "two closures never started"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(started.load(Ordering::Acquire), 2);

        let (open, waiting) = &*gate;
        *open.lock().unwrap() = true;
        waiting.notify_all();
        for handle in running {
            handle.await.unwrap().unwrap();
        }
        assert_eq!(started.load(Ordering::Acquire), 3);
    }

    #[test]
    fn canonical_signing_envelope_has_a_stable_golden() {
        let envelope = signing_envelope(
            &"dev.teamarness.fixture".parse().unwrap(),
            &WorkspaceId::new("00112233445566778899aabbccddeeff").unwrap(),
            "fixture.read",
            b"hello",
        )
        .unwrap();
        let golden: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/signing-envelope.golden.json"
        ))
        .unwrap();
        assert_eq!(
            encode_hex(&envelope),
            golden["envelopeHex"].as_str().unwrap()
        );
    }

    #[test]
    fn registry_accepts_a_neutral_test_only_extension() {
        let mut registry = Registry::production();
        registry.register(Descriptor {
            identity: "example.neutral",
            major: 1,
            minor: 0,
            kind: CapabilityKind::Neutral,
        });
        let selected = registry
            .negotiate(&CapabilityRequirement {
                identity: "example.neutral".into(),
                major: 1,
                min_minor: 0,
                max_minor: 0,
            })
            .unwrap();
        assert_eq!(selected.identity, "example.neutral");
    }
}
