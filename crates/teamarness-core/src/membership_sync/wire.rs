//! The `teamarness/membership-sync/1` frame codec.
//!
//! A frame is a four-byte big-endian length followed by a postcard-encoded
//! [`MembershipSyncFrame`]. The length is checked against the caller's cap
//! before a single byte of the body is allocated or read.

use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::MembershipOperationId;
use crate::membership::envelope::MAX_ENVELOPE_BYTES;
use crate::membership::replication::MAX_REPLICATION_OPERATIONS;

/// The wire carries bare digests; everything above the codec uses the typed
/// identifier, so a set on one side is never compared with bytes on the other.
pub(crate) fn wire_ids(ids: &[MembershipOperationId]) -> Vec<[u8; 32]> {
    ids.iter().map(|id| *id.as_bytes()).collect()
}

pub(crate) fn typed_ids(ids: &[[u8; 32]]) -> Vec<MembershipOperationId> {
    ids.iter()
        .map(|bytes| MembershipOperationId::from_hash(p2panda_core::Hash::from_bytes(*bytes)))
        .collect()
}

/// The first frame carries at most one whole advertised operation-ID set.
pub(crate) const FIRST_FRAME_MAX: usize = 64 * 1024;
/// Every later frame carries at most one canonical envelope.
pub(crate) const OPERATION_FRAME_MAX: usize = MAX_ENVELOPE_BYTES + 64;

/// The only `Refused` reason this protocol version defines.
pub(crate) const REFUSED_NOT_AUTHORIZED: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MembershipSyncFrame {
    Hello {
        workspace: [u8; 16],
        version: [u8; 32],
        operation_ids: Vec<[u8; 32]>,
    },
    Want {
        operation_ids: Vec<[u8; 32]>,
    },
    Operation {
        canonical_envelope: Vec<u8>,
    },
    EndOfOperations,
    Refused {
        reason: u8,
    },
    Done {
        version: [u8; 32],
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WireError {
    /// A declared frame length exceeds the cap for that position.
    TooLarge,
    /// The frame does not decode, or breaks a field rule.
    Malformed,
    /// The peer closed the stream.
    Closed,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("membership sync frame exceeds its bound"),
            Self::Malformed => formatter.write_str("malformed membership sync frame"),
            Self::Closed => formatter.write_str("membership sync stream closed"),
        }
    }
}

impl MembershipSyncFrame {
    /// Field rules that hold in both directions, checked on encode and decode.
    fn validate(&self) -> Result<(), WireError> {
        match self {
            Self::Hello { operation_ids, .. } | Self::Want { operation_ids } => {
                validate_ids(operation_ids)
            }
            Self::Operation { canonical_envelope } => {
                if canonical_envelope.is_empty() || canonical_envelope.len() > MAX_ENVELOPE_BYTES {
                    return Err(WireError::Malformed);
                }
                Ok(())
            }
            Self::EndOfOperations | Self::Refused { .. } | Self::Done { .. } => Ok(()),
        }
    }
}

/// Identifier sets are bounded, strictly ascending, and therefore duplicate
/// free, so a peer cannot inflate a set or hide work in its ordering.
fn validate_ids(operation_ids: &[[u8; 32]]) -> Result<(), WireError> {
    if operation_ids.len() > MAX_REPLICATION_OPERATIONS {
        return Err(WireError::TooLarge);
    }
    if operation_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(WireError::Malformed);
    }
    Ok(())
}

pub(crate) fn encode_frame(
    frame: &MembershipSyncFrame,
    max_bytes: usize,
) -> Result<Vec<u8>, WireError> {
    frame.validate()?;
    let bytes = postcard::to_stdvec(frame).map_err(|_| WireError::Malformed)?;
    if bytes.len() > max_bytes {
        return Err(WireError::TooLarge);
    }
    Ok(bytes)
}

/// The minimal reader the codec needs, so the length check can be proven
/// against an in-memory source as well as a QUIC stream.
pub(crate) trait FrameReader {
    fn read_frame_bytes(
        &mut self,
        buffer: &mut [u8],
    ) -> impl Future<Output = Result<(), WireError>> + Send;
}

impl FrameReader for iroh::endpoint::RecvStream {
    async fn read_frame_bytes(&mut self, buffer: &mut [u8]) -> Result<(), WireError> {
        self.read_exact(buffer).await.map_err(|_| WireError::Closed)
    }
}

/// Reads one frame, refusing an oversized declared length before allocating.
pub(crate) async fn read_frame<R: FrameReader + Send>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<MembershipSyncFrame, WireError> {
    let mut header = [0; 4];
    reader.read_frame_bytes(&mut header).await?;
    let length = u32::from_be_bytes(header) as usize;
    if length > max_bytes {
        return Err(WireError::TooLarge);
    }
    let mut bytes = vec![0; length];
    reader.read_frame_bytes(&mut bytes).await?;
    let frame: MembershipSyncFrame =
        postcard::from_bytes(&bytes).map_err(|_| WireError::Malformed)?;
    frame.validate()?;
    Ok(frame)
}

pub(crate) async fn write_frame(
    send: &mut iroh::endpoint::SendStream,
    frame: &MembershipSyncFrame,
    max_bytes: usize,
) -> Result<(), WireError> {
    let bytes = encode_frame(frame, max_bytes)?;
    let length = u32::try_from(bytes.len()).map_err(|_| WireError::TooLarge)?;
    send.write_all(&length.to_be_bytes())
        .await
        .map_err(|_| WireError::Closed)?;
    send.write_all(&bytes).await.map_err(|_| WireError::Closed)
}

#[cfg(test)]
pub(crate) struct SliceReader {
    bytes: Vec<u8>,
    pub(crate) position: usize,
}

#[cfg(test)]
impl SliceReader {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, position: 0 }
    }
}

#[cfg(test)]
impl FrameReader for SliceReader {
    async fn read_frame_bytes(&mut self, buffer: &mut [u8]) -> Result<(), WireError> {
        let end = self
            .position
            .checked_add(buffer.len())
            .ok_or(WireError::Closed)?;
        if end > self.bytes.len() {
            return Err(WireError::Closed);
        }
        buffer.copy_from_slice(&self.bytes[self.position..end]);
        self.position = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(frame: &MembershipSyncFrame) -> Vec<u8> {
        let bytes = postcard::to_stdvec(frame).unwrap();
        let mut framed = u32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
        framed.extend_from_slice(&bytes);
        framed
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let frames = [
            MembershipSyncFrame::Hello {
                workspace: [1; 16],
                version: [2; 32],
                operation_ids: vec![[3; 32], [4; 32]],
            },
            MembershipSyncFrame::Want {
                operation_ids: vec![[5; 32]],
            },
            MembershipSyncFrame::Operation {
                canonical_envelope: vec![9; 128],
            },
            MembershipSyncFrame::EndOfOperations,
            MembershipSyncFrame::Refused {
                reason: REFUSED_NOT_AUTHORIZED,
            },
            MembershipSyncFrame::Done { version: [6; 32] },
        ];
        for frame in frames {
            let mut reader = SliceReader::new(framed(&frame));
            assert_eq!(
                read_frame(&mut reader, FIRST_FRAME_MAX).await.unwrap(),
                frame
            );
        }
    }

    #[tokio::test]
    async fn oversized_first_frame_is_refused_before_allocation() {
        let mut bytes = u32::try_from(FIRST_FRAME_MAX + 1)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(&[0; 16]);
        let mut reader = SliceReader::new(bytes);

        assert_eq!(
            read_frame(&mut reader, FIRST_FRAME_MAX).await,
            Err(WireError::TooLarge)
        );
        assert_eq!(
            reader.position, 4,
            "only the length header was read; no body was allocated"
        );
    }

    #[tokio::test]
    async fn unsorted_or_duplicate_ids_are_rejected() {
        for ids in [vec![[2; 32], [1; 32]], vec![[1; 32], [1; 32]]] {
            let frame = MembershipSyncFrame::Want {
                operation_ids: ids.clone(),
            };
            assert_eq!(
                encode_frame(&frame, FIRST_FRAME_MAX),
                Err(WireError::Malformed)
            );
            let mut reader = SliceReader::new(framed(&frame));
            assert_eq!(
                read_frame(&mut reader, FIRST_FRAME_MAX).await,
                Err(WireError::Malformed)
            );
        }
    }

    #[tokio::test]
    async fn more_than_1024_ids_are_rejected() {
        let operation_ids = (0..=u32::try_from(MAX_REPLICATION_OPERATIONS).unwrap())
            .map(|index| {
                let mut id = [0; 32];
                id[..4].copy_from_slice(&index.to_be_bytes());
                id
            })
            .collect::<Vec<_>>();
        let frame = MembershipSyncFrame::Want { operation_ids };
        assert_eq!(
            encode_frame(&frame, FIRST_FRAME_MAX),
            Err(WireError::TooLarge)
        );
        let mut reader = SliceReader::new(framed(&frame));
        assert_eq!(
            read_frame(&mut reader, FIRST_FRAME_MAX).await,
            Err(WireError::TooLarge)
        );
    }
}
