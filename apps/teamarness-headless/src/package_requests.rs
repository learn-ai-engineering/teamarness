//! Non-rendered ordinary Host leases. The control task owns each lease through cleanup.
use crate::administration::host_error;
use crate::{ControlError, ErrorCode};
use serde_json::Value;
use teamarness_package_host::{
    CloseUiMount, OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome,
    PackageInstanceKey, UiRequest, UiRequestId,
};

pub(crate) async fn request(
    host: &PackageHost,
    instance: PackageInstanceKey,
    operation: String,
    digest: String,
    payload: Value,
) -> Result<Value, ControlError> {
    let opened = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(instance)))
        .await
        .map_err(host_error)?;
    let PackageHostOutcome::UiMountOpened { mount, loader, .. } = opened else {
        return Err(ControlError::new(ErrorCode::Internal));
    };
    let result = if loader.contract_digest != digest {
        Err(ControlError::new(ErrorCode::ContractDigestMismatch))
    } else {
        // The existing Host validates requests and responses against this lease's admission.
        let payload =
            serde_json::from_value(payload).map_err(|_| ControlError::new(ErrorCode::InvalidInput));
        match payload {
            Ok(payload) => {
                let input = UiRequest {
                    mount: mount.clone(),
                    request: UiRequestId::new("operator-request").expect("static request ID"),
                    operation,
                    payload,
                };
                match host.validate_ui_request(&input).await.map_err(host_error) {
                    Err(error) => Err(error),
                    Ok(()) => match host.execute(PackageHostCommand::UiRequest(input)).await {
                        Ok(PackageHostOutcome::UiResponse { payload }) => {
                            serde_json::to_value(payload).map_err(|_| uncertain_internal())
                        }
                        Ok(_) => Err(uncertain_internal()),
                        Err(envelope) => {
                            let mut error = host_error(envelope);
                            // The existing UI envelope omits WireFailure's dispatch uncertainty.
                            // After validation, failures cannot prove no effect. Keep its details
                            // unchanged and strengthen only the local observer verdict.
                            // Even a late Package terminal error may carry lost uncertainty.
                            error.outcome_uncertain = true;
                            error.retryable = false;
                            Err(error)
                        }
                    },
                }
            }
            Err(error) => Err(error),
        }
    };
    let cleanup = host
        .execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
        .await
        .map_err(host_error);
    // Never erase the original Host/Package error or its uncertainty with a cleanup error.
    match (result, cleanup) {
        (Err(mut error), Err(cleanup)) => {
            error.outcome_uncertain |= cleanup.outcome_uncertain;
            if error.outcome_uncertain {
                error.retryable = false;
            }
            Err(error)
        }
        (result @ Err(_), _) => result,
        (Ok(_), Err(mut error)) => {
            error.outcome_uncertain = true;
            error.retryable = false;
            Err(error)
        }
        (result, Ok(_)) => result,
    }
}

fn uncertain_internal() -> ControlError {
    let mut error = ControlError::new(ErrorCode::Internal);
    error.outcome_uncertain = true;
    error
}
