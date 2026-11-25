//! Parent side of the extension host connection: starts the host process on Bun or Node,
//! speaks `PersistentProtocol` over a Unix socket, and runs the Ready / init data / Initialized handshake.

pub mod activation_events;
pub mod console;
pub mod extension_development;
pub mod extension_management;
pub mod extension_validator;
pub mod frame;
pub mod gallery;
pub mod main_thread_bulk_edits;
pub mod main_thread_configuration;
pub mod main_thread_files;
pub mod main_thread_secret_state;
pub mod main_thread_storage;
pub mod main_thread_workspace;
pub mod proxy_ids;
pub mod request;
pub mod rpc;
pub mod scan;
pub mod main_thread;
pub mod session;
pub mod settings_file_system;
pub mod sign;
pub mod utc;
pub mod zip;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frame::{Frame, FrameReader, FrameType, encode_frame};

/// The JavaScript runtime the extension host runs on. Node is the control for Bun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Bun,
    Node,
}

impl Runtime {
    pub fn executable(self) -> &'static str {
        match self {
            Runtime::Bun => "bun",
            Runtime::Node => "node",
        }
    }

    fn runtime_args(self) -> &'static [&'static str] {
        match self {
            // Node 22.10+ resolves require() of ESM-only packages through "module-sync" (git needs file-type); Bun 1.4.5 only with the flag.
            Runtime::Bun => &["--conditions=module-sync"],
            Runtime::Node => &[],
        }
    }
}

/// One-byte messages of extensionHostProtocol.ts.
const MESSAGE_INITIALIZED: u8 = 1;
const MESSAGE_READY: u8 = 2;
const MESSAGE_TERMINATE: u8 = 3;

pub fn proxy_id(name: &str) -> u8 {
    let index = proxy_ids::PROXY_NAMES.iter().position(|n| *n == name).unwrap_or_else(|| panic!("unknown proxy {name}"));
    u8::try_from(index + 1).expect("more than 255 proxies")
}

pub fn proxy_name(rpc_id: u8) -> &'static str {
    proxy_ids::PROXY_NAMES.get(rpc_id as usize - 1).copied().unwrap_or("<unknown proxy>")
}

pub enum Incoming {
    Message(Vec<u8>),
    Closed,
}

struct WriterState {
    stream: UnixStream,
    outgoing_id: u32,
    incoming_id: u32,
}

/// A started extension host. Dropping it sends Terminate and kills the process.
pub struct ExtensionHost {
    pub child: Child,
    writer: Arc<Mutex<WriterState>>,
    incoming: Receiver<Incoming>,
    socket_path: PathBuf,
}

pub struct HostOptions<'a> {
    pub runtime: Runtime,
    /// out-exthost/bootstrap-exthost.js
    pub entry: &'a Path,
    pub extra_env: Vec<(String, String)>,
    pub extra_args: Vec<String>,
}

impl ExtensionHost {
    /// Starts the host and completes the handshake with `init_data` (an `IExtensionHostInitData`).
    pub fn start(options: HostOptions, init_data: &serde_json::Value) -> anyhow::Result<Self> {
        let socket_path = std::env::temp_dir().join(format!("vscode-gpui-exthost-{}-{}.sock", std::process::id(), unique()));
        // A killed earlier process with the same (reused) pid leaves its socket file; bind fails with EADDRINUSE.
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;

        let mut command = Command::new(options.runtime.executable());
        command
            .args(options.runtime.runtime_args())
            .args(options.extra_args)
            .arg(options.entry)
            .arg("--skipWorkspaceStorageLock")
            .env("VSCODE_EXTHOST_IPC_HOOK", &socket_path)
            .env("VSCODE_HANDLES_UNCAUGHT_ERRORS", "true")
            .envs(options.extra_env);
        let child = command.spawn()?;

        let stream = accept_with_timeout(&listener, Duration::from_secs(30))?;
        let reader_stream = stream.try_clone()?;
        let writer = Arc::new(Mutex::new(WriterState { stream, outgoing_id: 0, incoming_id: 0 }));
        let (sender, incoming) = channel();
        spawn_reader(reader_stream, writer.clone(), sender);

        let host = ExtensionHost { child, writer, incoming, socket_path };
        host.expect_one_byte(MESSAGE_READY)?;
        host.send(serde_json::to_vec(init_data)?);
        host.expect_one_byte(MESSAGE_INITIALIZED)?;
        Ok(host)
    }

    fn expect_one_byte(&self, expected: u8) -> anyhow::Result<()> {
        match self.incoming.recv_timeout(Duration::from_secs(30))? {
            Incoming::Message(bytes) if bytes == [expected] => Ok(()),
            Incoming::Message(bytes) => anyhow::bail!("expected handshake message {expected}, got {bytes:?}"),
            Incoming::Closed => anyhow::bail!("extension host closed the socket during the handshake"),
        }
    }

    /// Sends one Regular message (an RPC message after the handshake).
    pub fn send(&self, data: Vec<u8>) {
        let mut writer = self.writer.lock().unwrap();
        writer.outgoing_id += 1;
        let frame = Frame { kind: FrameType::Regular, id: writer.outgoing_id, ack: writer.incoming_id, data };
        write_frame(&mut writer.stream, &frame);
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        self.incoming.recv_timeout(timeout).ok()
    }

    pub fn try_recv(&self) -> Option<Incoming> {
        self.incoming.try_recv().ok()
    }
}

impl Drop for ExtensionHost {
    fn drop(&mut self) {
        self.send(vec![MESSAGE_TERMINATE]);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn spawn_reader(mut stream: UnixStream, writer: Arc<Mutex<WriterState>>, sender: Sender<Incoming>) {
    std::thread::Builder::new()
        .name("exthost-reader".into())
        .spawn(move || {
            let mut reader = FrameReader::default();
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                let n = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                reader.push(&chunk[..n]);
                let mut received_regular = false;
                while let Ok(Some(frame)) = reader.next_frame() {
                    if frame.kind != FrameType::Regular {
                        continue;
                    }
                    let mut state = writer.lock().unwrap();
                    if frame.id != state.incoming_id + 1 {
                        continue;
                    }
                    state.incoming_id = frame.id;
                    drop(state);
                    received_regular = true;
                    if sender.send(Incoming::Message(frame.data)).is_err() {
                        return;
                    }
                }
                // Upstream acks within 2 s; on a local socket an immediate ack per read costs nothing.
                if received_regular {
                    let mut state = writer.lock().unwrap();
                    let ack = Frame { kind: FrameType::Ack, id: 0, ack: state.incoming_id, data: Vec::new() };
                    write_frame(&mut state.stream, &ack);
                }
            }
            let _ = sender.send(Incoming::Closed);
        })
        .expect("spawn exthost reader thread");
}

fn write_frame(stream: &mut UnixStream, frame: &Frame) {
    let mut bytes = Vec::with_capacity(frame::HEADER_LENGTH + frame.data.len());
    encode_frame(frame, &mut bytes);
    // A failed write means the host is gone; the reader thread reports Closed.
    let _ = stream.write_all(&bytes);
}

fn accept_with_timeout(listener: &UnixListener, timeout: Duration) -> anyhow::Result<UnixStream> {
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                anyhow::ensure!(std::time::Instant::now() < deadline, "extension host did not connect within {timeout:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
