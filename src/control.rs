use std::{fs, io::{BufRead, BufReader, Write}, os::unix::net::{UnixListener, UnixStream}, path::{Path, PathBuf}, sync::mpsc, thread};

use anyhow::{Context, Result};

use crate::cli::{control_socket_path, parse_open_command};

#[derive(Clone, Debug)]
pub enum ControlCommand {
    Open { path: PathBuf, line: Option<usize> },
    Line(usize),
    TabNext,
    TabPrev,
    Recenter,
}

pub struct ControlServer {
    path: PathBuf,
    _thread: thread::JoinHandle<()>,
}

impl ControlServer {
    pub fn start(root: &Path, tx: mpsc::Sender<ControlCommand>) -> Result<Self> {
        // Listen for newline-delimited control messages from remote `piv`.
        let path = control_socket_path(root);
        if path.exists() { let _ = fs::remove_file(&path); }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind control socket {}", path.display()))?;
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    if let Some(command) = parse_control_message(&line) {
                        let _ = tx.send(command);
                    }
                }
            }
        });
        Ok(Self { path, _thread: thread })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) { let _ = fs::remove_file(&self.path); }
}

pub fn send_control_command(root: &Path, command: &ControlCommand) -> Result<()> {
    // Serialize one command and push it into the running viewer's socket.
    let socket_path = control_socket_path(root);
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("no running piv for {} at {}", root.display(), socket_path.display()))?;
    stream.write_all(format!("{}\n", encode_control_command(command)).as_bytes())?;
    Ok(())
}

pub fn encode_control_command(command: &ControlCommand) -> String {
    match command {
        ControlCommand::Open { path, line } => match line {
            Some(line) => format!("open {}:{}", path.display(), line),
            None => format!("open {}", path.display()),
        },
        ControlCommand::Line(line) => format!("line {line}"),
        ControlCommand::TabNext => "next-tab".to_string(),
        ControlCommand::TabPrev => "prev-tab".to_string(),
        ControlCommand::Recenter => "recenter".to_string(),
    }
}

pub fn parse_control_message(line: &str) -> Option<ControlCommand> {
    // Decode the tiny text protocol used by the control socket.
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("open ") { return parse_open_command(rest).ok(); }
    if let Some(rest) = line.strip_prefix("line ") { return rest.parse().ok().map(ControlCommand::Line); }
    match line {
        "next-tab" => Some(ControlCommand::TabNext),
        "prev-tab" => Some(ControlCommand::TabPrev),
        "recenter" => Some(ControlCommand::Recenter),
        _ => None,
    }
}
