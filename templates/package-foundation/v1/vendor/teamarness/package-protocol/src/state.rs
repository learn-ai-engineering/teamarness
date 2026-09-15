use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{Frame, FrameKind, INITIAL_STREAM_CREDIT, MAX_IN_FLIGHT_EACH_DIRECTION, ProtocolError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    HostToPackage,
    PackageToHost,
}

impl Direction {
    fn opposite(self) -> Self {
        match self {
            Self::HostToPackage => Self::PackageToHost,
            Self::PackageToHost => Self::HostToPackage,
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            Self::HostToPackage => "h:",
            Self::PackageToHost => "p:",
        }
    }
}

#[derive(Debug)]
struct StreamState {
    sender: Direction,
    credit: usize,
    last_sequence: u64,
}

#[derive(Debug, Default)]
pub struct ContractState {
    pending: BTreeMap<String, Direction>,
    terminal: BTreeSet<String>,
    streams: BTreeMap<String, StreamState>,
}

impl ContractState {
    pub fn observe(&mut self, direction: Direction, frame: &Frame) -> Result<(), ProtocolError> {
        match frame.kind() {
            FrameKind::Initialize | FrameKind::Initialized => self.observe_version(frame),
            FrameKind::Request => self.observe_request(direction, frame),
            FrameKind::Response | FrameKind::Error => self.observe_terminal(direction, frame),
            FrameKind::Cancel => self.observe_cancel(direction, frame),
            FrameKind::StreamOpen => self.observe_stream_open(direction, frame),
            FrameKind::Event => self.observe_event(direction, frame),
            FrameKind::Credit => self.observe_credit(direction, frame),
            FrameKind::StreamClose => self.observe_stream_close(frame),
            FrameKind::Ping | FrameKind::Pong | FrameKind::Shutdown | FrameKind::ShutdownAck => {
                Ok(())
            }
        }
    }

    fn observe_version(&self, frame: &Frame) -> Result<(), ProtocolError> {
        let major = frame
            .field("protocol")
            .and_then(Value::as_object)
            .and_then(|value| value.get("major"))
            .and_then(Value::as_u64)
            .ok_or_else(|| ProtocolError::new("malformed_frame"))?;
        if major != 1 {
            return Err(ProtocolError::new("incompatible_version"));
        }
        Ok(())
    }

    fn observe_request(
        &mut self,
        direction: Direction,
        frame: &Frame,
    ) -> Result<(), ProtocolError> {
        let id = id(frame, "id")?;
        validate_origin(id, direction)?;
        if self.pending.contains_key(id) || self.terminal.contains(id) {
            return Err(ProtocolError::new("duplicate_id"));
        }
        let in_flight = self
            .pending
            .values()
            .filter(|pending_direction| **pending_direction == direction)
            .count();
        if in_flight >= MAX_IN_FLIGHT_EACH_DIRECTION {
            return Err(ProtocolError::new("overloaded"));
        }
        self.pending.insert(id.to_owned(), direction);
        Ok(())
    }

    fn observe_terminal(
        &mut self,
        direction: Direction,
        frame: &Frame,
    ) -> Result<(), ProtocolError> {
        let id = id(frame, "id")?;
        if self.terminal.contains(id) {
            return Err(ProtocolError::new("second_terminal_response"));
        }
        let request_direction = self
            .pending
            .get(id)
            .copied()
            .ok_or_else(|| ProtocolError::new("unknown_response"))?;
        if request_direction == direction {
            return Err(ProtocolError::new("unknown_response"));
        }
        self.pending.remove(id);
        self.terminal.insert(id.to_owned());
        Ok(())
    }

    fn observe_cancel(&self, direction: Direction, frame: &Frame) -> Result<(), ProtocolError> {
        let control_id = id(frame, "id")?;
        validate_origin(control_id, direction)?;
        let target = id(frame, "targetId")?;
        if self.pending.get(target) != Some(&direction) {
            return Err(ProtocolError::new("unknown_request"));
        }
        Ok(())
    }

    fn observe_stream_open(
        &mut self,
        direction: Direction,
        frame: &Frame,
    ) -> Result<(), ProtocolError> {
        let id = id(frame, "id")?;
        validate_origin(id, direction)?;
        if self.streams.contains_key(id) {
            return Err(ProtocolError::new("duplicate_id"));
        }
        let credit = frame
            .field("initialCredit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| ProtocolError::new("malformed_frame"))?;
        if credit != INITIAL_STREAM_CREDIT {
            return Err(ProtocolError::new("credit_violation"));
        }
        self.streams.insert(
            id.to_owned(),
            StreamState {
                sender: direction.opposite(),
                credit,
                last_sequence: 0,
            },
        );
        Ok(())
    }

    fn observe_event(&mut self, direction: Direction, frame: &Frame) -> Result<(), ProtocolError> {
        let stream_id = id(frame, "streamId")?;
        let stream = self
            .streams
            .get_mut(stream_id)
            .ok_or_else(|| ProtocolError::new("unknown_stream"))?;
        if stream.sender != direction {
            return Err(ProtocolError::new("unknown_stream"));
        }
        if stream.credit == 0 {
            return Err(ProtocolError::new("credit_violation"));
        }
        let sequence = decimal(frame, "sequence")?;
        if sequence != stream.last_sequence.saturating_add(1) {
            return Err(ProtocolError::new("event_sequence"));
        }
        stream.credit -= 1;
        stream.last_sequence = sequence;
        Ok(())
    }

    fn observe_credit(&mut self, direction: Direction, frame: &Frame) -> Result<(), ProtocolError> {
        let stream_id = id(frame, "streamId")?;
        let stream = self
            .streams
            .get_mut(stream_id)
            .ok_or_else(|| ProtocolError::new("unknown_stream"))?;
        if stream.sender == direction {
            return Err(ProtocolError::new("unknown_stream"));
        }
        let amount = frame
            .field("amount")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=INITIAL_STREAM_CREDIT).contains(value))
            .ok_or_else(|| ProtocolError::new("credit_violation"))?;
        stream.credit = stream
            .credit
            .checked_add(amount)
            .ok_or_else(|| ProtocolError::new("credit_violation"))?;
        Ok(())
    }

    fn observe_stream_close(&mut self, frame: &Frame) -> Result<(), ProtocolError> {
        let stream_id = id(frame, "streamId")?;
        self.streams
            .remove(stream_id)
            .ok_or_else(|| ProtocolError::new("unknown_stream"))?;
        Ok(())
    }
}

fn id<'a>(frame: &'a Frame, field: &str) -> Result<&'a str, ProtocolError> {
    frame
        .field(field)
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 128)
        .ok_or_else(|| ProtocolError::new("malformed_frame"))
}

fn validate_origin(id: &str, direction: Direction) -> Result<(), ProtocolError> {
    if !id.starts_with(direction.id_prefix()) || id.len() <= 2 {
        return Err(ProtocolError::new("invalid_id_namespace"));
    }
    if !id[2..]
        .bytes()
        .all(|value| value.is_ascii_alphanumeric() || b"._~-".contains(&value))
    {
        return Err(ProtocolError::new("invalid_id_namespace"));
    }
    Ok(())
}

fn decimal(frame: &Frame, field: &str) -> Result<u64, ProtocolError> {
    let text = frame
        .field(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError::new("invalid_decimal"))?;
    if text != "0" && text.starts_with('0') || !text.bytes().all(|value| value.is_ascii_digit()) {
        return Err(ProtocolError::new("invalid_decimal"));
    }
    text.parse()
        .map_err(|_| ProtocolError::new("invalid_decimal"))
}
