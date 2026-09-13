use std::str::FromStr;

use iroh_base::{CustomAddr, EndpointAddr, PublicKey, TransportAddr};
use iroh_tickets::endpoint::EndpointTicket;
use resonance_core::WorkspaceInvitation;

const TOKEN: &str = "01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c";
const INVITER: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";
const TICKET: &str = "endpointadveu3dd4kofecv66vihwezoyx4zkr3wv27l464siipou2iui3jcyaqac5uhi5dqom5c6l3smvwgc6jomv4gc3lqnrss4lybab7qaaab4e6a";
const GOLDEN: &str = "resonance-workspace-v1.0100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c0074656e64706f696e746164766575336464346b6f66656376363676696877657a6f7978347a6b7233777632376c343634736969706f7532697569336a637961716163357568693564716f6d3563366c33736d76776763366a6f6d76346763336c716e727373346c7962616237716161616234653661";

fn encode_transfer(token: &str, ticket: &str, suffix: &[u8]) -> String {
    let token = decode_hex(token);
    let mut payload = Vec::new();
    payload.push(1);
    payload.extend_from_slice(&u16::try_from(token.len()).unwrap().to_be_bytes());
    payload.extend_from_slice(&token);
    payload.extend_from_slice(&u16::try_from(ticket.len()).unwrap().to_be_bytes());
    payload.extend_from_slice(ticket.as_bytes());
    payload.extend_from_slice(suffix);
    format!("resonance-workspace-v1.{}", encode_hex(&payload))
}

fn ticket_with_addresses(count: usize, identity: &str) -> String {
    let identity = PublicKey::from_str(identity).unwrap();
    let addresses = (0..count)
        .map(|index| TransportAddr::Ip(format!("127.0.0.1:{}", 10_000 + index).parse().unwrap()));
    EndpointTicket::new(EndpointAddr::from_parts(identity, addresses)).to_string()
}

#[test]
fn outer_transfer_golden_vector_is_stable_and_opaque() {
    let invitation = WorkspaceInvitation::from_str(GOLDEN).unwrap();

    assert_eq!(invitation.to_string(), GOLDEN);
    assert_eq!(invitation.encoded(), GOLDEN);
    assert_eq!(encode_transfer(TOKEN, TICKET, &[]), GOLDEN);
    assert!(!GOLDEN.contains("private"));
}

#[test]
fn transfer_rejects_non_canonical_shape_versions_and_trailing_bytes() {
    assert!(WorkspaceInvitation::from_str("resonance-workspace-v2.00").is_err());
    assert!(WorkspaceInvitation::from_str("resonance-workspace-v1.0").is_err());
    assert!(WorkspaceInvitation::from_str(&GOLDEN.to_uppercase()).is_err());
    assert!(WorkspaceInvitation::from_str(&encode_transfer(TOKEN, TICKET, &[0])).is_err());

    let mut unsupported = decode_hex(GOLDEN.strip_prefix("resonance-workspace-v1.").unwrap());
    unsupported[0] = 2;
    let unsupported = format!("resonance-workspace-v1.{}", encode_hex(&unsupported));
    assert!(WorkspaceInvitation::from_str(&unsupported).is_err());

    let mut payload = decode_hex(GOLDEN.strip_prefix("resonance-workspace-v1.").unwrap());
    payload[1..3].copy_from_slice(&195_u16.to_be_bytes());
    assert!(
        WorkspaceInvitation::from_str(&format!("resonance-workspace-v1.{}", encode_hex(&payload)))
            .is_err()
    );

    let ticket_length_offset = 1 + 2 + 196;
    let ticket_length = u16::try_from(TICKET.len()).unwrap();
    payload = decode_hex(GOLDEN.strip_prefix("resonance-workspace-v1.").unwrap());
    payload[ticket_length_offset..ticket_length_offset + 2]
        .copy_from_slice(&(ticket_length + 1).to_be_bytes());
    assert!(
        WorkspaceInvitation::from_str(&format!("resonance-workspace-v1.{}", encode_hex(&payload)))
            .is_err()
    );
}

#[test]
fn transfer_enforces_text_payload_ticket_and_address_limits() {
    let text_over_limit = format!("resonance-workspace-v1.{}", "00".repeat(4_085));
    assert!(text_over_limit.len() > 8 * 1024);
    assert!(WorkspaceInvitation::from_str(&text_over_limit).is_err());

    let payload_over_limit = format!("resonance-workspace-v1.{}", "00".repeat(4_097));
    assert!(WorkspaceInvitation::from_str(&payload_over_limit).is_err());

    let ticket_over_limit = "x".repeat(3 * 1024 + 1);
    assert!(
        WorkspaceInvitation::from_str(&encode_transfer(TOKEN, &ticket_over_limit, &[])).is_err()
    );

    let eight = ticket_with_addresses(8, INVITER);
    assert!(WorkspaceInvitation::from_str(&encode_transfer(TOKEN, &eight, &[])).is_ok());
    let nine = ticket_with_addresses(9, INVITER);
    assert!(WorkspaceInvitation::from_str(&encode_transfer(TOKEN, &nine, &[])).is_err());

    let inviter = PublicKey::from_str(INVITER).unwrap();
    let custom = EndpointTicket::new(EndpointAddr::from_parts(
        inviter,
        [TransportAddr::Custom(CustomAddr::from_parts(
            42, b"address",
        ))],
    ))
    .to_string();
    assert!(WorkspaceInvitation::from_str(&encode_transfer(TOKEN, &custom, &[])).is_err());
}

#[test]
fn transfer_rejects_ticket_identity_that_differs_from_signed_inviter() {
    let wrong_ticket = ticket_with_addresses(
        1,
        "1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca",
    );
    assert!(WorkspaceInvitation::from_str(&encode_transfer(TOKEN, &wrong_ticket, &[])).is_err());
}

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
