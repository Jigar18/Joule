use serde_json::Value;
use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const FOCUS_EVENT: &[u8] = b"activewindowv2>>";

pub struct Window {
    pub pid: u32,
    pub class: String,
}

pub struct Hyprland {
    socket_dir: PathBuf,
    events: UnixStream,
    unfinished_event: Vec<u8>,
}

impl Hyprland {
    pub fn connect() -> io::Result<Self> {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| missing("XDG_RUNTIME_DIR"))?;
        let instance = env::var_os("HYPRLAND_INSTANCE_SIGNATURE").ok_or_else(|| missing("HYPRLAND_INSTANCE_SIGNATURE"))?;
        let socket_dir = PathBuf::from(runtime_dir).join("hypr").join(instance);

        let events = UnixStream::connect(socket_dir.join(".socket2.sock"))?;
        events.set_nonblocking(true)?;
        Ok(Self { socket_dir, events, unfinished_event: Vec::new() })
    }

    pub fn events_fd(&self) -> RawFd {
        self.events.as_raw_fd()
    }

    /// Consumes every pending event and reports whether focus moved to another window.
    pub fn focus_changed(&mut self) -> io::Result<bool> {
        let mut changed = false;
        let mut chunk = [0u8; 4096];
        loop {
            match self.events.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "Hyprland closed its event socket")),
                Ok(read) => {
                    self.unfinished_event.extend_from_slice(&chunk[..read]);
                    while let Some(newline) = self.unfinished_event.iter().position(|&byte| byte == b'\n') {
                        changed |= self.unfinished_event.starts_with(FOCUS_EVENT);
                        self.unfinished_event.drain(..=newline);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(changed),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub fn focused_window(&self) -> io::Result<Option<Window>> {
        Ok(window_from_json(&self.request("j/activewindow")?))
    }

    pub fn open_windows(&self) -> io::Result<Vec<Window>> {
        let windows = self.request("j/clients")?;
        Ok(windows.as_array().into_iter().flatten().filter_map(window_from_json).collect())
    }

    pub fn screen_is_on(&self) -> io::Result<bool> {
        let monitors = self.request("j/monitors")?;
        let lit = |monitor: &Value| monitor["dpmsStatus"] == true && monitor["disabled"] != true;
        Ok(monitors.as_array().is_some_and(|monitors| monitors.iter().any(lit)))
    }

    fn request(&self, command: &str) -> io::Result<Value> {
        let mut socket = UnixStream::connect(self.socket_dir.join(".socket.sock"))?;
        socket.write_all(command.as_bytes())?;
        let mut reply = String::new();
        socket.read_to_string(&mut reply)?;
        serde_json::from_str(&reply).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }
}

fn window_from_json(window: &Value) -> Option<Window> {
    Some(Window { pid: window["pid"].as_u64()? as u32, class: window["class"].as_str()?.to_owned() })
}

fn missing(variable: &str) -> io::Error {
    io::Error::new(ErrorKind::NotFound, format!("{variable} is not set; is Hyprland running?"))
}
