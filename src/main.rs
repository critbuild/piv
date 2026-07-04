use anyhow::{Context, Result};
use piv::{
    app::App,
    cli::{CliMode, parse_cli_mode},
    control::send_control_command,
    tracker_rpc::{send_tracker_rpc_request, serve_tracker_rpc},
    ui::run_tui,
};

fn main() -> Result<()> {
    // Dispatch: local watch mode vs remote control command.
    match parse_cli_mode()? {
        CliMode::Watch(root) => {
            let root = root.canonicalize().context("watch root must exist")?;
            run(root)
        }
        CliMode::Remote { root, command } => {
            let root = root.canonicalize().context("control root must exist")?;
            send_control_command(&root, &command)
        }
        CliMode::TrackerServe { socket, db } => serve_tracker_rpc(&socket, &db),
        CliMode::TrackerRpc { socket, request } => {
            println!("{}", send_tracker_rpc_request(&socket, &request)?);
            Ok(())
        }
    }
}

fn run(root: std::path::PathBuf) -> Result<()> {
    // Start TUI and hand the terminal to the app loop.
    run_tui(|terminal| {
        let mut app = App::new(root)?;
        app.run(terminal)
    })
}
