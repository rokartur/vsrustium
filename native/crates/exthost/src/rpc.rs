//! RPC messages of src/vs/workbench/services/extensions/common/rpcProtocol.ts.
//! Every message: type u8, request id u32 BE, then a body that depends on the type.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum MessageType {
    RequestJsonArgs = 1,
    RequestJsonArgsWithCancellation = 2,
    RequestMixedArgs = 3,
    RequestMixedArgsWithCancellation = 4,
    Acknowledged = 5,
    Cancel = 6,
    ReplyOkEmpty = 7,
    ReplyOkVsBuffer = 8,
    ReplyOkJson = 9,
    ReplyOkJsonWithBuffers = 10,
    ReplyErrError = 11,
    ReplyErrEmpty = 12,
}

const ARG_STRING: u8 = 1;
const ARG_VSBUFFER: u8 = 2;
const ARG_SERIALIZED_OBJECT_WITH_BUFFERS: u8 = 3;
const ARG_UNDEFINED: u8 = 4;

/// rpcProtocol.ts `undefinedRef`: an argument that is `undefined`, sent as `ArgType.Undefined`.
pub fn undefined() -> Value {
    serde_json::json!({ "$$ref$$": -1 })
}

impl Arg {
    pub fn from_value(value: Value) -> Arg {
        match value.get("$$ref$$").and_then(Value::as_i64) == Some(-1) {
            true => Arg::Undefined,
            false => Arg::Json(value),
        }
    }
}

/// One argument as it crosses the wire. `Json` is what most calls carry.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Json(Value),
    Buffer(Vec<u8>),
    /// JSON in which `{"$$ref$$": i}` stands for `buffers[i]`.
    JsonWithBuffers { json: Value, buffers: Vec<Vec<u8>> },
    Undefined,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Empty,
    Buffer(Vec<u8>),
    Json(Value),
    JsonWithBuffers { json: Value, buffers: Vec<Vec<u8>> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request { req: u32, rpc_id: u8, method: String, args: Vec<Arg>, cancellable: bool },
    Acknowledged { req: u32 },
    Cancel { req: u32 },
    ReplyOk { req: u32, value: Reply },
    /// `None` is ReplyErrEmpty; otherwise the serialized error (`{$isError, name, message, stack}`).
    ReplyErr { req: u32, error: Option<Value> },
}

pub fn encode(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    match message {
        Message::Request { req, rpc_id, method, args, cancellable } => {
            let mixed = args.iter().any(|arg| !matches!(arg, Arg::Json(_)));
            let kind = match (mixed, cancellable) {
                (false, false) => MessageType::RequestJsonArgs,
                (false, true) => MessageType::RequestJsonArgsWithCancellation,
                (true, false) => MessageType::RequestMixedArgs,
                (true, true) => MessageType::RequestMixedArgsWithCancellation,
            };
            header(&mut out, kind, *req);
            out.push(*rpc_id);
            let method = method.as_bytes();
            out.push(u8::try_from(method.len()).expect("method name longer than 255 bytes"));
            out.extend_from_slice(method);
            if mixed {
                write_mixed_args(&mut out, args);
            } else {
                let values: Vec<&Value> = args
                    .iter()
                    .map(|arg| match arg {
                        Arg::Json(value) => value,
                        _ => unreachable!(),
                    })
                    .collect();
                write_long(&mut out, serde_json::to_string(&values).unwrap().as_bytes());
            }
        }
        Message::Acknowledged { req } => header(&mut out, MessageType::Acknowledged, *req),
        Message::Cancel { req } => header(&mut out, MessageType::Cancel, *req),
        Message::ReplyOk { req, value } => match value {
            Reply::Empty => header(&mut out, MessageType::ReplyOkEmpty, *req),
            Reply::Buffer(buffer) => {
                header(&mut out, MessageType::ReplyOkVsBuffer, *req);
                write_long(&mut out, buffer);
            }
            Reply::Json(value) => {
                header(&mut out, MessageType::ReplyOkJson, *req);
                write_long(&mut out, serde_json::to_string(value).unwrap().as_bytes());
            }
            Reply::JsonWithBuffers { json, buffers } => {
                header(&mut out, MessageType::ReplyOkJsonWithBuffers, *req);
                out.extend_from_slice(&(buffers.len() as u32).to_be_bytes());
                write_long(&mut out, serde_json::to_string(json).unwrap().as_bytes());
                for buffer in buffers {
                    write_long(&mut out, buffer);
                }
            }
        },
        Message::ReplyErr { req, error: None } => header(&mut out, MessageType::ReplyErrEmpty, *req),
        Message::ReplyErr { req, error: Some(error) } => {
            header(&mut out, MessageType::ReplyErrError, *req);
            write_long(&mut out, serde_json::to_string(error).unwrap().as_bytes());
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<Message> {
    let mut r = Reader { bytes, offset: 0 };
    let kind = r.u8()?;
    let req = r.u32()?;
    Ok(match kind {
        1..=4 => {
            let rpc_id = r.u8()?;
            let method_len = r.u8()? as usize;
            let method = String::from_utf8(r.take(method_len)?.to_vec())?;
            let args = if kind <= 2 {
                let values: Vec<Value> = serde_json::from_slice(r.long()?)?;
                values.into_iter().map(Arg::Json).collect()
            } else {
                read_mixed_args(&mut r)?
            };
            let cancellable = kind == 2 || kind == 4;
            Message::Request { req, rpc_id, method, args, cancellable }
        }
        5 => Message::Acknowledged { req },
        6 => Message::Cancel { req },
        7 => Message::ReplyOk { req, value: Reply::Empty },
        8 => Message::ReplyOk { req, value: Reply::Buffer(r.long()?.to_vec()) },
        9 => Message::ReplyOk { req, value: Reply::Json(serde_json::from_slice(r.long()?)?) },
        10 => {
            let count = r.u32()?;
            let mut json = serde_json::from_slice(r.long()?)?;
            drop_undefined_refs(&mut json);
            let mut buffers = Vec::new();
            for _ in 0..count {
                buffers.push(r.long()?.to_vec());
            }
            Message::ReplyOk { req, value: Reply::JsonWithBuffers { json, buffers } }
        }
        11 => Message::ReplyErr { req, error: Some(serde_json::from_slice(r.long()?)?) },
        12 => Message::ReplyErr { req, error: None },
        other => anyhow::bail!("unknown rpc message type {other}"),
    })
}

fn header(out: &mut Vec<u8>, kind: MessageType, req: u32) {
    out.push(kind as u8);
    out.extend_from_slice(&req.to_be_bytes());
}

fn write_long(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn write_mixed_args(out: &mut Vec<u8>, args: &[Arg]) {
    out.push(u8::try_from(args.len()).expect("more than 255 rpc arguments"));
    for arg in args {
        match arg {
            Arg::Json(value) => {
                out.push(ARG_STRING);
                write_long(out, serde_json::to_string(value).unwrap().as_bytes());
            }
            Arg::Buffer(buffer) => {
                out.push(ARG_VSBUFFER);
                write_long(out, buffer);
            }
            Arg::JsonWithBuffers { json, buffers } => {
                out.push(ARG_SERIALIZED_OBJECT_WITH_BUFFERS);
                out.extend_from_slice(&(buffers.len() as u32).to_be_bytes());
                write_long(out, serde_json::to_string(json).unwrap().as_bytes());
                for buffer in buffers {
                    write_long(out, buffer);
                }
            }
            Arg::Undefined => out.push(ARG_UNDEFINED),
        }
    }
}

/// `parseJsonAndRestoreBufferRefs`: `{"$$ref$$": -1}` is how `stringifyJsonWithBufferRefs` keeps `undefined`; the reviver
/// turns it back into a missing key, or `undefined` (here `null`) in an array.
fn drop_undefined_refs(value: &mut Value) {
    let is_undefined = |value: &Value| value.get("$$ref$$").and_then(Value::as_i64) == Some(-1);
    match value {
        Value::Object(map) => {
            map.retain(|_, value| !is_undefined(value));
            map.values_mut().for_each(drop_undefined_refs);
        }
        Value::Array(items) => {
            for item in items {
                if is_undefined(item) {
                    *item = Value::Null;
                }
                drop_undefined_refs(item);
            }
        }
        _ => {}
    }
}

fn read_mixed_args(r: &mut Reader) -> anyhow::Result<Vec<Arg>> {
    let count = r.u8()?;
    let mut args = Vec::with_capacity(count as usize);
    for _ in 0..count {
        args.push(match r.u8()? {
            ARG_STRING => Arg::Json(serde_json::from_slice(r.long()?)?),
            ARG_VSBUFFER => Arg::Buffer(r.long()?.to_vec()),
            ARG_SERIALIZED_OBJECT_WITH_BUFFERS => {
                let buffer_count = r.u32()?;
                let mut json = serde_json::from_slice(r.long()?)?;
                drop_undefined_refs(&mut json);
                let mut buffers = Vec::new();
                for _ in 0..buffer_count {
                    buffers.push(r.long()?.to_vec());
                }
                Arg::JsonWithBuffers { json, buffers }
            }
            ARG_UNDEFINED => Arg::Undefined,
            other => anyhow::bail!("unknown rpc argument type {other}"),
        });
    }
    Ok(args)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.offset + len;
        anyhow::ensure!(end <= self.bytes.len(), "rpc message truncated");
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }

    fn long(&mut self) -> anyhow::Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_every_message_kind() {
        let messages = [
            Message::Request { req: 1, rpc_id: 9, method: "$test_latency".into(), args: vec![Arg::Json(json!(42))], cancellable: false },
            Message::Request {
                req: 2,
                rpc_id: 3,
                method: "$x".into(),
                args: vec![Arg::Undefined, Arg::Buffer(vec![1, 2]), Arg::JsonWithBuffers { json: json!({"$$ref$$": 0}), buffers: vec![vec![7]] }],
                cancellable: true,
            },
            Message::Acknowledged { req: 3 },
            Message::Cancel { req: 4 },
            Message::ReplyOk { req: 5, value: Reply::Empty },
            Message::ReplyOk { req: 6, value: Reply::Buffer(vec![0, 255]) },
            Message::ReplyOk { req: 7, value: Reply::Json(json!({"a": [1, "ż"]})) },
            Message::ReplyOk { req: 8, value: Reply::JsonWithBuffers { json: json!([{"$$ref$$": 0}]), buffers: vec![vec![9, 9]] } },
            Message::ReplyErr { req: 9, error: Some(json!({"$isError": true, "message": "boom"})) },
            Message::ReplyErr { req: 10, error: None },
        ];
        for message in messages {
            assert_eq!(decode(&encode(&message)).unwrap(), message);
        }
    }

    /// Bytes produced by upstream's MessageIO for `$test_latency(42)` with req 1 on rpc id 9.
    #[test]
    fn matches_upstream_json_request_layout() {
        let bytes = encode(&Message::Request { req: 1, rpc_id: 9, method: "$m".into(), args: vec![Arg::Json(json!(42))], cancellable: false });
        assert_eq!(bytes, [&[1u8, 0, 0, 0, 1, 9, 2][..], b"$m", &[0, 0, 0, 4], b"[42]"].concat());
    }

    /// `{ oldResource: undefined, newResource }` as `stringifyJsonWithBufferRefs` writes it.
    #[test]
    fn undefined_refs_become_missing_keys() {
        let json = json!({ "oldResource": {"$$ref$$": -1}, "newResource": {"path": "/a"}, "list": [{"$$ref$$": -1}, {"$$ref$$": 0}] });
        let message = Message::Request { req: 1, rpc_id: 2, method: "$m".into(), args: vec![Arg::JsonWithBuffers { json, buffers: vec![vec![1]] }], cancellable: false };
        let Message::Request { args, .. } = decode(&encode(&message)).unwrap() else { panic!("request expected") };
        assert_eq!(args[0], Arg::JsonWithBuffers { json: json!({ "newResource": {"path": "/a"}, "list": [null, {"$$ref$$": 0}] }), buffers: vec![vec![1]] });
    }

    #[test]
    fn an_undefined_argument_is_sent_as_undefined() {
        assert_eq!(Arg::from_value(undefined()), Arg::Undefined);
        assert_eq!(Arg::from_value(json!(1)), Arg::Json(json!(1)));
    }
}
