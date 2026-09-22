//! The one crossing between the two Ed25519 stacks in the dependency graph.
//!
//! Iroh carries `ed25519-dalek` 3 and owns [`PublicIdentity`]; p2panda carries
//! `ed25519-dalek` 2 and owns operation authorship. Neither can name the
//! other's key type, so a key crosses as its 32 compressed Edwards bytes.
//! Every crossing goes through this module, and the tests below pin what makes
//! that sound: the byte encoding round trips, a signature made under one stack
//! verifies under the other, and the two `from_bytes` guards accept and reject
//! the same keys. A crossing is fail-closed — bytes one stack refuses never
//! become a key on the other side.
//!
//! The duplicate stacks are an upstream constraint, recorded in
//! `context/.delta/DELTA-009-duplicate-crypto-and-sqlite-stacks.md`.

use p2panda_core::VerifyingKey;

use crate::PublicIdentity;

/// The p2panda author key for a Teamarness identity, or `None` if p2panda's
/// guard refuses bytes that iroh's accepted.
pub(crate) fn p2panda_verifying_key(identity: PublicIdentity) -> Option<VerifyingKey> {
    VerifyingKey::from_bytes(identity.as_bytes()).ok()
}

/// The Teamarness identity for a p2panda author key, or `None` if iroh's guard
/// refuses bytes that p2panda's accepted.
pub(crate) fn identity(author: &VerifyingKey) -> Option<PublicIdentity> {
    PublicIdentity::from_bytes(*author.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use iroh_base::{PublicKey, SecretKey};
    use p2panda_core::SigningKey;

    use super::*;

    const MESSAGE: &[u8] = b"teamarness key bridge fixed message";

    /// Ed25519 encodings that are on the boundary of what a decompression
    /// guard accepts: the identity, the four order-2 and order-4 points, two
    /// canonical order-8 points, and a non-canonical `y` (`y == p`).
    const EDGE_ENCODINGS: [[u8; 32]; 8] = [
        [0; 32],
        [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0x80,
        ],
        [
            0xec, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0x7f,
        ],
        [
            0x26, 0xe8, 0x95, 0x8f, 0xc2, 0xb2, 0x27, 0xb0, 0x45, 0xc3, 0xf4, 0x89, 0xf2, 0xef,
            0x98, 0xf0, 0xd5, 0xdf, 0xac, 0x05, 0xd3, 0xc6, 0x33, 0x39, 0xb1, 0x38, 0x02, 0x88,
            0x6d, 0x53, 0xfc, 0x05,
        ],
        [
            0xc7, 0x17, 0x6a, 0x70, 0x3d, 0x4d, 0xd8, 0x4f, 0xba, 0x3c, 0x0b, 0x76, 0x0d, 0x10,
            0x67, 0x0f, 0x2a, 0x20, 0x53, 0xfa, 0x2c, 0x39, 0xcc, 0xc6, 0x4e, 0xc7, 0xfd, 0x77,
            0x92, 0xac, 0x03, 0x7a,
        ],
        [
            0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0x7f,
        ],
        [0xff; 32],
    ];

    fn random_secret_bytes() -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("test randomness");
        bytes
    }

    #[test]
    fn a_key_keeps_its_bytes_across_the_bridge() {
        for _ in 0..32 {
            let secret = SecretKey::from_bytes(&random_secret_bytes());
            let identity_bytes = *secret.public().as_bytes();
            let identity_value = PublicIdentity::from_bytes(identity_bytes).expect("an iroh key");

            let crossed = p2panda_verifying_key(identity_value).expect("p2panda accepts the key");
            assert_eq!(crossed.as_bytes(), &identity_bytes);

            let back = identity(&crossed).expect("iroh accepts the key");
            assert_eq!(back, identity_value);
            assert_eq!(back.as_bytes(), &identity_bytes);
        }
    }

    #[test]
    fn a_signature_made_under_one_stack_verifies_under_the_other() {
        for _ in 0..32 {
            let secret_bytes = random_secret_bytes();
            let iroh_secret = SecretKey::from_bytes(&secret_bytes);
            let p2panda_secret = SigningKey::from_bytes(&secret_bytes);

            // The same private bytes name the same public key on both sides.
            assert_eq!(
                iroh_secret.public().as_bytes(),
                p2panda_secret.verifying_key().as_bytes()
            );

            let identity_value =
                PublicIdentity::from_bytes(*iroh_secret.public().as_bytes()).expect("an iroh key");
            let crossed = p2panda_verifying_key(identity_value).expect("p2panda accepts the key");

            // iroh signs, p2panda verifies.
            let iroh_signature = iroh_secret.sign(MESSAGE);
            assert!(crossed.verify(
                MESSAGE,
                &p2panda_core::Signature::from_bytes(&iroh_signature.to_bytes())
            ));

            // p2panda signs, iroh verifies.
            let p2panda_signature = p2panda_secret.sign(MESSAGE);
            iroh_secret
                .public()
                .verify(
                    MESSAGE,
                    &iroh_base::Signature::from_bytes(&p2panda_signature.to_bytes()),
                )
                .expect("iroh accepts the p2panda signature");

            // A tampered message is refused on both sides.
            let mut other = MESSAGE.to_vec();
            other[0] ^= 0x01;
            assert!(!crossed.verify(
                &other,
                &p2panda_core::Signature::from_bytes(&iroh_signature.to_bytes())
            ));
            assert!(
                iroh_secret
                    .public()
                    .verify(
                        &other,
                        &iroh_base::Signature::from_bytes(&p2panda_signature.to_bytes()),
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn the_two_guards_agree_and_the_bridge_is_fail_closed() {
        let mut disagreements = 0;
        for bytes in EDGE_ENCODINGS {
            let iroh_accepts = PublicKey::from_bytes(&bytes).is_ok();
            let p2panda_accepts = VerifyingKey::from_bytes(&bytes).is_ok();
            if iroh_accepts != p2panda_accepts {
                disagreements += 1;
            }

            // A key crosses only when both stacks accept its bytes. That holds
            // whether or not the guards agree, so a future upstream divergence
            // cannot smuggle a key past one of them.
            let crossed = if iroh_accepts {
                p2panda_verifying_key(PublicIdentity::from_bytes(bytes).expect("iroh accepts it"))
                    .is_some()
            } else if p2panda_accepts {
                identity(&VerifyingKey::from_bytes(&bytes).expect("p2panda accepts it")).is_some()
            } else {
                false
            };
            assert_eq!(
                crossed,
                iroh_accepts && p2panda_accepts,
                "the bridge must accept {bytes:02x?} only when both stacks do"
            );
        }

        // Today the two guards are the same decompression check on either side
        // of the duplicate dependency, so they never disagree. The assertion
        // above is what keeps a divergence fail-closed if that changes.
        assert_eq!(
            disagreements, 0,
            "the two ed25519 stacks now disagree about an edge encoding; see DELTA-009"
        );
    }
}
