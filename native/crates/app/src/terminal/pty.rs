//! node-pty's unix `spawn` (pty.cc `PtyFork`): the shell runs in a new session whose controlling terminal is a fresh
//! PTY; its output is read on a thread and its exit is waited for on another.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use futures::channel::mpsc::UnboundedSender;

pub enum PtyEvent {
    Data(Vec<u8>),
    /// The process exited, with its exit code; `None` when a signal ended it.
    Exit(Option<i32>),
}

pub struct Pty {
    master: File,
    pub pid: i32,
}

impl Pty {
    pub fn spawn(executable: &Path, args: &[String], cwd: &Path, env: &HashMap<String, String>, cols: u16, rows: u16, events: UnboundedSender<PtyEvent>) -> io::Result<Pty> {
        let (master, slave) = open_pty(cols, rows)?;
        let master_fd = master.as_raw_fd();
        let mut command = Command::new(executable);
        // node-pty unixTerminal.ts sets PWD, so the shell shows the path as given, not the resolved /private/var one.
        command.args(args).env_clear().envs(env).env("PWD", cwd).current_dir(cwd);
        command.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                libc::close(master_fd);
                // pty.cc resets the signals the parent may have changed.
                for signal in [libc::SIGCHLD, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGALRM, libc::SIGPIPE] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        let pid = child.id() as i32;
        let master = File::from(master);
        let mut reader = master.try_clone()?;
        let data_events = events.clone();
        let (read_done, read_finished) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new().name("pty-read".into()).spawn(move || {
            let _read_done = read_done;
            let mut buffer = vec![0; 64 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if data_events.unbounded_send(PtyEvent::Data(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
        std::thread::Builder::new().name("pty-wait".into()).spawn(move || {
            let mut child = child;
            let code = child.wait().ok().and_then(|status| status.code());
            // unixTerminal.ts emits exit once the pty socket closes, or after DESTROY_SOCKET_TIMEOUT_MS (200 ms).
            let _ = read_finished.recv_timeout(std::time::Duration::from_millis(200));
            let _ = events.unbounded_send(PtyEvent::Exit(code));
        })?;
        Ok(Pty { master, pid })
    }

    pub fn write(&mut self, data: &[u8]) {
        // A write after the shell exited fails with EIO; node-pty drops it the same way.
        let _ = self.master.write_all(data);
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    }

    /// node-pty `kill(signal)`, SIGHUP by default.
    pub fn kill(&self, signal: i32) {
        unsafe { libc::kill(self.pid, signal) };
    }

    /// node-pty `process` (pty.cc `pty_getproc`): the name of the terminal's foreground process group leader.
    pub fn foreground_process(&self) -> Option<String> {
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        if group <= 0 {
            return None;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe { libc::proc_pidinfo(group, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        if read != size {
            return None;
        }
        let name = unsafe { CStr::from_ptr(info.pbi_comm.as_ptr()) };
        Some(name.to_string_lossy().into_owned())
    }

    /// The current directory of the shell (terminalProcess.ts `getCwd`, which asks `lsof` on macOS).
    pub fn cwd(&self) -> Option<String> {
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        let read = unsafe { libc::proc_pidinfo(self.pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
        if read != size {
            return None;
        }
        let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
        Some(path.to_string_lossy().into_owned())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.kill(libc::SIGHUP);
    }
}

fn open_pty(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave) = (0, 0);
    let mut size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // pty.cc sets IUTF8 so the line discipline erases whole UTF-8 characters.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(master.as_raw_fd(), &mut termios) } == 0 {
        termios.c_iflag |= libc::IUTF8;
        unsafe { libc::tcsetattr(master.as_raw_fd(), libc::TCSANOW, &termios) };
    }
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((master, slave))
}
