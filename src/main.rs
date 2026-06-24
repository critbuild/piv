use anyhow::{Context, Result};
use piv::{app::App, cli::{parse_cli_mode, CliMode}, control::send_control_command, ui::run_tui};

fn main() -> Result<()> {
    match parse_cli_mode()? {
        CliMode::Watch(root) => {
            let root = root.canonicalize().context("watch root must exist")?;
            run(root)
        }
        CliMode::Remote { root, command } => {
            let root = root.canonicalize().context("control root must exist")?;
            send_control_command(&root, &command)
        }
    }
}

fn run(root: std::path::PathBuf) -> Result<()> {
    run_tui(|terminal| {
        let mut app = App::new(root)?;
        app.run(terminal)
    })
}
