//! The lawh app: window, input, status line and command line.

mod app;
mod cli;
mod view;

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context as _, anyhow};
use lawh_core::Board;
use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoop};

use crate::cli::Command;

/// Exit status for bad arguments, as most command-line tools use.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let started = Instant::now();
    init_logging();
    let command = match cli::parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            cli::print_usage_error(&err);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match command {
        Command::Version => cli::print_version(),
        Command::Help => cli::print_help(),
        Command::Open(path) => {
            if let Err(err) = run(&path, started) {
                cli::print_error(&err);
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Logs go to stderr, filtered by `RUST_LOG` (default `warn`), for example `RUST_LOG=debug lawh`.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Opens `path` in a window and runs until the window is closed.
fn run(path: &Path, started: Instant) -> anyhow::Result<()> {
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "lawh starting");
    // Read the board first, so a bad file is reported without opening a window.
    let board = load_board(path)?;
    tracing::info!(
        path = %path.display(),
        objects = board.len(),
        "board loaded"
    );

    let event_loop = EventLoop::new().context("could not open a window")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = app::App::new(board, window_title(path), started);
    event_loop.run_app(&mut app).context("the window failed")?;
    app.into_result()
}

/// Reads and parses a board. Errors read `<path>: <io error>` or `<path>:<line>:<col>: <message>`.
fn load_board(path: &Path) -> anyhow::Result<Board> {
    let src = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    // FormatError's message already starts with `line:column:`.
    lawh_core::from_lawh(&src).map_err(|err| anyhow!("{}:{err}", path.display()))
}

/// `<file name> - lawh`.
fn window_title(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    format!("{name} - lawh")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_file_name() {
        let path = Path::new("boards").join("area.lawh");
        assert_eq!(window_title(&path), "area.lawh - lawh");
    }

    #[test]
    fn title_falls_back_to_the_whole_path() {
        assert_eq!(window_title(Path::new("..")), ".. - lawh");
    }
}
