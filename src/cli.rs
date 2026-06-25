use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}, path::{Path, PathBuf}};

use anyhow::{bail, Context, Result};

use crate::control::ControlCommand;

pub enum CliMode {
    Watch(PathBuf),
    Remote { root: PathBuf, command: ControlCommand },
}

pub fn parse_cli_mode() -> Result<CliMode> {
    // Parse either a watch root or a single remote control command.
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut watch_root: Option<PathBuf> = None;
    let mut command: Option<ControlCommand> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().context("--root needs a path")?)),
            "--open" => command = Some(parse_open_command(&args.next().context("--open needs a target")?)?),
            "--highlight" => command = Some(parse_highlight_command(&args.next().context("--highlight needs a target")?)?),
            "--line" => command = Some(ControlCommand::Line(args.next().context("--line needs a line number")?.parse().context("line must be a number")?)),
            "--next-tab" => command = Some(ControlCommand::TabNext),
            "--prev-tab" => command = Some(ControlCommand::TabPrev),
            "--recenter" => command = Some(ControlCommand::Recenter),
            _ if arg.starts_with('-') => bail!("unknown flag: {arg}"),
            _ => {
                if command.is_some() || watch_root.is_some() { bail!("unexpected extra arg: {arg}"); }
                watch_root = Some(PathBuf::from(arg));
            }
        }
    }

    if let Some(command) = command {
        Ok(CliMode::Remote { root: root.unwrap_or(std::env::current_dir()?), command })
    } else {
        Ok(CliMode::Watch(watch_root.unwrap_or(std::env::current_dir()?)))
    }
}

pub fn parse_open_command(target: &str) -> Result<ControlCommand> {
    let (path, line) = split_target_line(target);
    Ok(ControlCommand::Open { path: PathBuf::from(path), line })
}

pub fn parse_highlight_command(target: &str) -> Result<ControlCommand> {
    let (path, line) = split_target_line(target);
    let line = line.context("--highlight target must include a line, like path/to/file.rs:120")?;
    Ok(ControlCommand::Highlight { path: PathBuf::from(path), line })
}

pub fn split_target_line(target: &str) -> (&str, Option<usize>) {
    if let Some((path, line)) = target.rsplit_once(':') {
        if let Ok(line) = line.parse::<usize>() { return (path, Some(line)); }
    }
    (target, None)
}

pub fn control_socket_path(root: &Path) -> PathBuf {
    // Stable socket path derived from the watched root.
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    std::env::temp_dir().join(format!("piv-{:016x}.sock", hasher.finish()))
}
