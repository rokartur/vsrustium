//! Framing of `PersistentProtocol` from src/vs/base/parts/ipc/common/ipc.net.ts.
//! Header: type u8, id u32 BE, ack u32 BE, data length u32 BE, then data.

pub const HEADER_LENGTH: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    None = 0,
    Regular = 1,
    Control = 2,
    Ack = 3,
    Disconnect = 5,
    ReplayRequest = 6,
    Pause = 7,
    Resume = 8,
    KeepAlive = 9,
}

impl FrameType {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::Regular,
            2 => Self::Control,
            3 => Self::Ack,
            5 => Self::Disconnect,
            6 => Self::ReplayRequest,
            7 => Self::Pause,
            8 => Self::Resume,
            9 => Self::KeepAlive,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameType,
    pub id: u32,
    pub ack: u32,
    pub data: Vec<u8>,
}

pub fn encode_frame(frame: &Frame, out: &mut Vec<u8>) {
    out.push(frame.kind as u8);
    out.extend_from_slice(&frame.id.to_be_bytes());
    out.extend_from_slice(&frame.ack.to_be_bytes());
    out.extend_from_slice(&(frame.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&frame.data);
}

/// Accumulates socket chunks and yields whole frames, like `ProtocolReader`.
#[derive(Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    pub fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        if self.buffer.len() < HEADER_LENGTH {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buffer[9..13].try_into()?) as usize;
        if self.buffer.len() < HEADER_LENGTH + len {
            return Ok(None);
        }
        let kind = FrameType::from_u8(self.buffer[0])
            .ok_or_else(|| anyhow::anyhow!("unknown frame type {}", self.buffer[0]))?;
        let id = u32::from_be_bytes(self.buffer[1..5].try_into()?);
        let ack = u32::from_be_bytes(self.buffer[5..9].try_into()?);
        let data = self.buffer[HEADER_LENGTH..HEADER_LENGTH + len].to_vec();
        self.buffer.drain(..HEADER_LENGTH + len);
        Ok(Some(Frame { kind, id, ack, data }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_split_frames() {
        let frame = Frame { kind: FrameType::Regular, id: 7, ack: 3, data: b"hello".to_vec() };
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes);
        encode_frame(&Frame { kind: FrameType::KeepAlive, id: 0, ack: 0, data: vec![] }, &mut bytes);
        let mut reader = FrameReader::default();
        reader.push(&bytes[..5]);
        assert_eq!(reader.next_frame().unwrap(), None);
        reader.push(&bytes[5..]);
        assert_eq!(reader.next_frame().unwrap(), Some(frame));
        assert_eq!(reader.next_frame().unwrap().unwrap().kind, FrameType::KeepAlive);
        assert_eq!(reader.next_frame().unwrap(), None);
    }
}
