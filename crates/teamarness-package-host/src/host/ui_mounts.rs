//! UI mount commands, and the mount bookkeeping every UI command rests on.

use std::sync::Arc;

use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, host_error, instance_error,
};
use crate::values::{
    LifecycleState, PackageInstanceKey, UiMountId, UiResourceToken, UiSubscriptionId,
};
use crate::{
    CloseUiMount, MountLease, OpenUiMount, PackageHost, PackageHostOutcome, ReadUiResource,
    ReportUiFault, UiLoaderDescriptor, UiPackageContext, UiWorkspaceContext, instance,
};

#[allow(clippy::result_large_err)]
impl PackageHost {
    pub(crate) async fn open_ui_mount(
        &self,
        input: OpenUiMount,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _gate = self.inner.mount_management.lock().await;
        let actor = self.actor(&input.instance).await?;
        let replaced = {
            let data = self.inner.data.lock().await;
            data.mounts
                .iter()
                .filter(|(_, lease)| lease.instance == input.instance)
                .map(|(mount, _)| mount.clone())
                .collect::<Vec<_>>()
        };
        for mount in replaced {
            actor.close_mount(&mount).await;
            self.revoke_mount(&mount).await;
        }
        actor
            .activate()
            .await
            .map_err(|failure| instance_error(failure, &input.instance))?;
        let (mount, process_generation) = actor
            .open_mount()
            .await
            .map_err(|failure| instance_error(failure, &input.instance))?;
        let ui = self
            .admitted_ui(&input.instance.package_identity)
            .ok_or_else(|| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?;
        let token =
            random_resource_token().map_err(|code| host_error(code, PackageErrorStage::Ui))?;
        let package = self
            .inner
            .data
            .lock()
            .await
            .packages
            .iter()
            .find(|package| package.identity == input.instance.package_identity)
            .cloned()
            .ok_or_else(|| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?;
        let loader = UiLoaderDescriptor {
            token: token.clone(),
            entrypoint: ui.cache.entrypoint.clone(),
            ui_host_version: 1,
            contract_digest: ui.cache.contract_digest.clone(),
            package: UiPackageContext {
                identity: input.instance.package_identity.clone(),
                version: package.version,
                display_name: package.display_name,
                source: package.source,
                trust: package.trust,
                lifecycle: LifecycleState::Active,
                process_generation,
            },
            workspace: UiWorkspaceContext {
                id: input.instance.workspace_id.clone(),
                name: actor.workspace_name().to_owned(),
            },
        };
        self.inner.data.lock().await.mounts.insert(
            mount.clone(),
            MountLease {
                instance: input.instance,
                process_generation,
                token,
                ui,
            },
        );
        Ok(PackageHostOutcome::UiMountOpened {
            mount,
            loader,
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn close_ui_mount(
        &self,
        input: CloseUiMount,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _gate = self.inner.mount_management.lock().await;
        if let Some(actor) = self.actor_for_mount(&input.mount).await {
            actor.close_mount(&input.mount).await;
        }
        self.revoke_mount(&input.mount).await;
        Ok(PackageHostOutcome::UiMountClosed {
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn report_ui_fault(
        &self,
        input: ReportUiFault,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        self.fail_mount(&input.mount).await;
        Ok(PackageHostOutcome::UiFaultRecorded {
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn read_ui_resource(
        &self,
        input: ReadUiResource,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (lease, _) = self.live_mount(&input.mount).await?;
        if lease.token != input.token {
            return Err(host_error(
                PackageErrorCode::StaleMount,
                PackageErrorStage::Ui,
            ));
        }
        // Reading and hashing a UI resource is blocking file work.
        let ui = Arc::clone(&lease.ui);
        let relative_path = input.relative_path.clone();
        let resource = tokio::task::spawn_blocking(move || ui.cache.resolve(&relative_path))
            .await
            .map_err(|_| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?
            .map_err(|_| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?;
        Ok(PackageHostOutcome::UiResource {
            bytes: resource.bytes,
            media_type: resource.media_type,
        })
    }

    pub(crate) async fn actor_for_mount(
        &self,
        mount: &UiMountId,
    ) -> Option<Arc<instance::InstanceActor>> {
        let data = self.inner.data.lock().await;
        let lease = data.mounts.get(mount)?;
        data.instances.get(&lease.instance).cloned()
    }

    pub(crate) async fn live_mount(
        &self,
        mount: &UiMountId,
    ) -> Result<(MountLease, Arc<instance::InstanceActor>), PackageErrorEnvelope> {
        let (lease, actor) = {
            let data = self.inner.data.lock().await;
            let lease =
                data.mounts.get(mount).cloned().ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
            let actor = data
                .instances
                .get(&lease.instance)
                .cloned()
                .ok_or_else(|| host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui))?;
            (lease, actor)
        };
        if !actor
            .is_mount_current(mount, lease.process_generation)
            .await
        {
            self.revoke_mount(mount).await;
            return Err(host_error(
                PackageErrorCode::StaleMount,
                PackageErrorStage::Ui,
            ));
        }
        Ok((lease, actor))
    }

    pub(crate) async fn revoke_mount(&self, mount: &UiMountId) {
        let mut data = self.inner.data.lock().await;
        data.mounts.remove(mount);
        data.ui_routes
            .retain(|(route_mount, _), _| route_mount != mount);
    }

    pub(crate) async fn revoke_mounts_for_instance(&self, instance: &PackageInstanceKey) {
        let mounts = {
            let data = self.inner.data.lock().await;
            data.mounts
                .iter()
                .filter(|(_, lease)| lease.instance == *instance)
                .map(|(mount, _)| mount.clone())
                .collect::<Vec<_>>()
        };
        for mount in mounts {
            self.revoke_mount(&mount).await;
        }
    }

    /// Releases the Host's record of a stream the Package has ended.
    ///
    /// Nothing else removed it, so the acknowledgement for the last delivered
    /// event — which is routinely still in flight when the stream ends — found
    /// a live subscription and released credit for a stream the contract had
    /// already closed.
    pub(crate) async fn release_ui_subscription(
        &self,
        mount: &UiMountId,
        subscription: &UiSubscriptionId,
    ) {
        if let Some(actor) = self.actor_for_mount(mount).await {
            let _ = actor.close_subscription(mount, subscription).await;
        }
        self.inner
            .data
            .lock()
            .await
            .ui_routes
            .remove(&(mount.clone(), subscription.clone()));
    }

    pub(crate) async fn fail_mount(&self, mount: &UiMountId) {
        if let Some(actor) = self.actor_for_mount(mount).await
            && actor.fail_mount(mount).await
        {
            self.bump_revision().await;
        }
    }
}

pub(crate) fn random_resource_token() -> Result<UiResourceToken, PackageErrorCode> {
    let mut random = [0_u8; 24];
    getrandom::fill(&mut random).map_err(|_| PackageErrorCode::UiLoadFailed)?;
    use base64::Engine as _;
    UiResourceToken::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random))
        .map_err(|_| PackageErrorCode::UiLoadFailed)
}
