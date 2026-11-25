//! launchMainService.ts and main.ts `claimInstance`: the first instance listens on `<user data>/<version>-main.sock`; a
//! later start with the same user data hands its arguments and working directory to it and exits.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::App;
use serde_json::{Value, json};

/// What a second instance sends: its command line and working directory.
pub struct Launch {
    pub args: Vec<String>,
    pub cwd: PathBuf,
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
                let args = message["args"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str().map(str::to_owned)).collect();
                let cwd = PathBuf::from(message["cwd"].as_str().unwrap_or_default());
                if sender.unbounded_send(Launch { args, cwd }).is_err() {
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
            let _ = cx.update(|cx| crate::workbench::host::start_open_window(launch.args, launch.cwd, cx));
        }
    })
    .detach();
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
        let launch = futures::executor::block_on(launches.next()).unwrap();
        assert_eq!((launch.args, launch.cwd), (args, PathBuf::from("/tmp")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
