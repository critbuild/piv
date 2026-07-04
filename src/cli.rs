use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{control::ControlCommand, tracker::default_db_path, tracker_rpc::tracker_socket_path};

pub enum CliMode {
    Watch(PathBuf),
    Remote {
        root: PathBuf,
        command: ControlCommand,
    },
    TrackerServe {
        socket: PathBuf,
        db: PathBuf,
    },
    TrackerRpc {
        socket: PathBuf,
        request: String,
    },
}

pub fn parse_cli_mode() -> Result<CliMode> {
    parse_cli_mode_from(std::env::args().skip(1))
}

pub fn parse_cli_mode_from<I, S>(args: I) -> Result<CliMode>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    // Parse either a watch root, a remote control command, or tracker socket mode.
    let mut args = args.into_iter().map(Into::into);
    let mut root: Option<PathBuf> = None;
    let mut watch_root: Option<PathBuf> = None;
    let mut command: Option<ControlCommand> = None;
    let mut tracker_serve = false;
    let mut tracker_rpc: Option<String> = None;
    let mut tracker_socket: Option<PathBuf> = None;
    let mut tracker_db: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().context("--root needs a path")?)),
            "--open" => {
                command = Some(parse_open_command(
                    &args.next().context("--open needs a target")?,
                )?)
            }
            "--highlight" => {
                command = Some(parse_highlight_target(
                    &args.next().context("--highlight needs a target")?,
                )?)
            }
            "--highlight-range" => {
                command = Some(parse_highlight_target(
                    &args.next().context("--highlight-range needs a target")?,
                )?)
            }
            "--line" => {
                command = Some(ControlCommand::Line(
                    args.next()
                        .context("--line needs a line number")?
                        .parse()
                        .context("line must be a number")?,
                ))
            }
            "--next-tab" => command = Some(ControlCommand::TabNext),
            "--prev-tab" => command = Some(ControlCommand::TabPrev),
            "--recenter" => command = Some(ControlCommand::Recenter),
            "--tracker-serve" => tracker_serve = true,
            "--tracker-rpc" => {
                tracker_rpc = Some(args.next().context("--tracker-rpc needs a JSON request")?)
            }
            "--tracker-socket" => {
                tracker_socket = Some(PathBuf::from(
                    args.next().context("--tracker-socket needs a path")?,
                ))
            }
            "--tracker-db" => {
                tracker_db = Some(PathBuf::from(
                    args.next().context("--tracker-db needs a path")?,
                ))
            }
            _ if arg.starts_with('-') => bail!("unknown flag: {arg}"),
            _ => {
                if command.is_some()
                    || watch_root.is_some()
                    || tracker_serve
                    || tracker_rpc.is_some()
                {
                    bail!("unexpected extra arg: {arg}");
                }
                watch_root = Some(PathBuf::from(arg));
            }
        }
    }

    let tracker_mode_count = usize::from(tracker_serve) + usize::from(tracker_rpc.is_some());
    if tracker_mode_count > 0 && (command.is_some() || watch_root.is_some()) {
        bail!("tracker mode cannot be combined with watch or remote control mode");
    }
    if tracker_mode_count > 1 {
        bail!("choose only one tracker mode");
    }

    if tracker_serve {
        let db = match tracker_db {
            Some(db) => db,
            None => default_db_path()?,
        };
        return Ok(CliMode::TrackerServe {
            socket: tracker_socket.unwrap_or_else(tracker_socket_path),
            db,
        });
    }
    if let Some(request) = tracker_rpc {
        return Ok(CliMode::TrackerRpc {
            socket: tracker_socket.unwrap_or_else(tracker_socket_path),
            request,
        });
    }
    if let Some(command) = command {
        Ok(CliMode::Remote {
            root: root.unwrap_or(std::env::current_dir()?),
            command,
        })
    } else {
        Ok(CliMode::Watch(
            watch_root.unwrap_or(std::env::current_dir()?),
        ))
    }
}

pub fn parse_open_command(target: &str) -> Result<ControlCommand> {
    let (path, line) = split_target_line(target);
    Ok(ControlCommand::Open {
        path: PathBuf::from(path),
        line,
    })
}

pub fn parse_highlight_target(target: &str) -> Result<ControlCommand> {
    let (path, start_line, end_line) = parse_highlight_range(target)?;
    Ok(ControlCommand::Highlight {
        path,
        start_line,
        end_line,
    })
}

fn parse_highlight_range(target: &str) -> Result<(PathBuf, usize, usize)> {
    let (path, range) = target.rsplit_once(':').context("highlight target must include a line or range, like path/to/file.rs:120 or path/to/file.rs:120-140")?;
    let (start_line, end_line) = match range.split_once('-') {
        Some((start, end)) => (
            start
                .parse()
                .context("highlight start line must be a number")?,
            end.parse().context("highlight end line must be a number")?,
        ),
        None => {
            let line = range.parse().context("highlight line must be a number")?;
            (line, line)
        }
    };
    Ok((PathBuf::from(path), start_line, end_line))
}

// Parse a remote-control target, optionally with a trailing :line.
pub fn split_target_line(target: &str) -> (&str, Option<usize>) {
    if let Some((path, line)) = target.rsplit_once(':') {
        if let Ok(line) = line.parse::<usize>() {
            return (path, Some(line));
        }
    }
    (target, None)
}

pub fn control_socket_path(root: &Path) -> PathBuf {
    // Stable socket path derived from the watched root.
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    std::env::temp_dir().join(format!("piv-{:016x}.sock", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracker_socket_modes() {
        match parse_cli_mode_from(["--tracker-serve"]).unwrap() {
            CliMode::TrackerServe { .. } => {}
            _ => panic!("expected tracker serve mode"),
        }
        match parse_cli_mode_from(["--tracker-rpc", "{\"method\":\"project.list\"}"]).unwrap() {
            CliMode::TrackerRpc { request, .. } => assert!(request.contains("project.list")),
            _ => panic!("expected tracker rpc mode"),
        }
    }

    #[test]
    fn parses_highlight_range_target() {
        let command = parse_highlight_target("src/control.rs:24-34").unwrap();
        match command {
            ControlCommand::Highlight {
                path,
                start_line,
                end_line,
            } => {
                assert_eq!(path, PathBuf::from("src/control.rs"));
                assert_eq!(start_line, 24);
                assert_eq!(end_line, 34);
            }
            _ => panic!("expected highlight command"),
        }
    }
}
