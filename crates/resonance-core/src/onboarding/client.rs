use std::sync::Arc;

use iroh_base::PublicKey;

use super::transfer::VerifiedWorkspaceInvitation;
use super::transport::Transport;
use super::wire::{
    JoinRequest, MAX_RESPONSE_BODY_BYTES, SuccessResponse, decode_error, decode_success,
    encode_request,
};
use crate::OnboardingError;
use crate::invitation::VerifiedInvitationClaim;

pub(crate) async fn join(
    transport: &Arc<Transport>,
    invitation: VerifiedWorkspaceInvitation,
    claim: VerifiedInvitationClaim,
) -> Result<SuccessResponse, OnboardingError> {
    transport
        .whole_join(exchange(transport, invitation, claim))
        .await
}

async fn exchange(
    transport: &Arc<Transport>,
    invitation: VerifiedWorkspaceInvitation,
    claim: VerifiedInvitationClaim,
) -> Result<SuccessResponse, OnboardingError> {
    let expected_peer = invitation.token.inviter.as_public_key();
    if invitation.endpoint_addr.id != expected_peer {
        return Err(OnboardingError::Authentication);
    }
    let (connection, _permit) = transport.connect(invitation.endpoint_addr).await?;
    require_peer(connection.remote_id(), expected_peer)?;
    let (mut send, mut receive) = transport
        .response_io(async {
            connection
                .open_bi()
                .await
                .map_err(|_| OnboardingError::Authentication)
        })
        .await?;
    let request =
        encode_request(&JoinRequest { claim }).map_err(|_| OnboardingError::InvalidInput)?;
    #[allow(unused_mut)]
    let mut frame = transport
        .response_io(async {
            send.write_all(&request)
                .await
                .map_err(|_| OnboardingError::Authentication)?;
            send.finish().map_err(|_| OnboardingError::Authentication)?;
            receive
                .read_to_end(MAX_RESPONSE_BODY_BYTES + 4)
                .await
                .map_err(|_| OnboardingError::Overload)
        })
        .await?;
    #[cfg(test)]
    transport.alter_received_bootstrap(&mut frame);
    if let Ok(response) = decode_success(&frame) {
        return Ok(response);
    }
    if let Ok(response) = decode_error(&frame) {
        return Err(response.error);
    }
    Err(OnboardingError::InvalidBootstrap)
}

fn require_peer(actual: PublicKey, expected: PublicKey) -> Result<(), OnboardingError> {
    if actual == expected {
        Ok(())
    } else {
        Err(OnboardingError::Authentication)
    }
}
