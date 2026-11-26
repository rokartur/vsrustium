//! launchMainService.ts and main.ts `claimInstance`: the first instance listens on `<user data>/<version>-main.sock`; a
//! later start with the same user data hands its arguments and working directory to it and exits.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::App;
use serde_json::{Value, json};

/// What a second instance sends: its command line and working directory, or a `--status` request answered with the
/// main process diagnostics (main.ts asks the `diagnostics` channel of the same socket).
pub enum Launch {
    Open { args: Vec<String>, cwd: PathBuf },
    Status(futures::channel::oneshot::Sender<crate::diagnostics::MainDiagnostics>),
}

pub fn socket_path(user_data: &Path) -> PathBuf {
    user_data.join(format!("{}-main.sock", crate::VERSION))
}

/// A second instance: true once the running one took the arguments. False when no instance listens (a stale socket is
/// removed then, as main.ts does on `ECONNREFUSED`).
pub fn send_to_running_instance(socket: &Path, args: &[String], cwd: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        let _ = std::fs::remove_file(socket);
        return false;
    };
    let message = json!({ "args": args, "cwd": cwd });
    if writeln!(stream, "{message}").is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok_and(|read| read > 0)
}

/// `diagnosticsMainService.getMainDiagnostics` of the instance listening on `socket`; None when none listens.
pub fn request_main_diagnostics(socket: &Path) -> Option<crate::diagnostics::MainDiagnostics> {
    let mut stream = UnixStream::connect(socket).ok()?;
    writeln!(stream, "{}", json!({ "status": true })).ok()?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).ok()?;
    serde_json::from_str(&answer).ok()
}

/// The first instance: each second instance's launch, answered once it is read.
pub fn listen(socket: &Path) -> Option<UnboundedReceiver<Launch>> {
    let listener = UnixListener::bind(socket).inspect_err(|error| eprintln!("[launch] listening on {}: {error}", socket.display())).ok()?;
    let (sender, receiver) = unbounded();
    std::thread::Builder::new()
        .name("launch".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message["status"] == true {
                    let (reply, answer) = futures::channel::oneshot::channel();
                    if sender.unbounded_send(Launch::Status(reply)).is_err() {
                        return;
                    }
                    if let Ok(diagnostics) = futures::executor::block_on(answer) {
                        let _ = writeln!(&stream, "{}", json!(diagnostics));
                    }
                    continue;
                }
                let args = message["args"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str().map(str::to_owned)).collect();
                let cwd = PathBuf::from(message["cwd"].as_str().unwrap_or_default());
                if sender.unbounded_send(Launch::Open { args, cwd }).is_err() {
                    return;
                }
                let _ = (&stream).write_all(b"ok\n");
            }
        })
        .expect("spawn the launch thread");
    Some(receiver)
}

/// Opens each launch until the app quits.
pub fn handle_launches(mut launches: UnboundedReceiver<Launch>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(launch) = launches.next().await {
            let _ = cx.update(|cx| match launch {
                Launch::Open { args, cwd } => crate::workbench::host::start_open_window(args, cwd, cx),
                Launch::Status(reply) => drop(reply.send(main_diagnostics(cx))),
            });
        }
    })
    .detach();
}

/// `getMainDiagnostics`: the open windows with their titles and folders, and the extension hosts by name.
fn main_diagnostics(cx: &mut App) -> crate::diagnostics::MainDiagnostics {
    let main_pid = std::process::id();
    let mut windows = Vec::new();
    let mut screen_reader = false;
    for handle in cx.windows().into_iter().filter_map(|handle| handle.downcast::<crate::workbench::Workbench>()) {
        let _ = handle.update(cx, |workbench, window, _| {
            screen_reader |= window.is_a11y_active();
            let folders = workbench.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default();
            windows.push(crate::diagnostics::WindowDiagnostics { id: handle.window_id().as_u64(), pid: main_pid, title: workbench.window_title(), folders });
        });
    }
    // utilityProcess.ts names a window's host `extension-host [<window id>]`; the port does not keep the host's pid
    // per window, so the hosts are numbered in start (pid) order.
    let hosts = crate::ps::list_processes(main_pid, 0.).map(|root| root.children).unwrap_or_default();
    let pid_to_names = hosts.iter().filter(|child| child.cmd.contains("bootstrap-exthost.js")).enumerate().map(|(index, host)| (host.pid, format!("extension-host [{}]", index + 1))).collect();
    crate::diagnostics::MainDiagnostics { main_pid, main_arguments: std::env::args().skip(1).collect(), windows, pid_to_names, screen_reader }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_instance_hands_its_arguments_to_the_first() {
        let dir = std::env::temp_dir().join(format!("vsrustium-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = socket_path(&dir);
        assert!(!send_to_running_instance(&socket, &[], &dir), "nothing listens yet");
        std::fs::write(&socket, "").unwrap();
        assert!(!send_to_running_instance(&socket, &[], &dir));
        assert!(!socket.exists(), "a stale socket file is removed");
        let mut launches = listen(&socket).unwrap();
        let args = vec!["vsrustium".to_owned(), "folder".to_owned()];
        assert!(send_to_running_instance(&socket, &args, Path::new("/tmp")));
        let Some(Launch::Open { args: sent, cwd }) = futures::executor::block_on(launches.next()) else { panic!("an open launch") };
        assert_eq!((sent, cwd), (args, PathBuf::from("/tmp")));
        let request = std::thread::spawn({
            let socket = socket.clone();
            move || request_main_diagnostics(&socket)
        });
        let Some(Launch::Status(reply)) = futures::executor::block_on(launches.next()) else { panic!("a status request") };
        reply.send(crate::diagnostics::MainDiagnostics { main_pid: 7, ..Default::default() }).unwrap();
        assert_eq!(request.join().unwrap().map(|info| info.main_pid), Some(7));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
