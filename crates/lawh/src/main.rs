//! The lawh app: opens a `.lawh` board in a window and draws it, with mouse pan and zoom. With
//! `--math-worker` it runs as the math worker instead.

mod app;
mod cli;
mod view;

use std::fs::File;
use std::io::{IsTerminal as _, Read as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context as _, anyhow};
use lawh_core::format::MAX_FILE_BYTES;
use lawh_core::{Board, FormatError};
use lawh_math::{MathEngine, WorkerCommand};
use lawh_render::Renderer;
use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoop};

use crate::cli::Command;

/// Exit status for bad arguments, as most command-line tools use.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    // lawh compiles a board's math in a copy of itself started with this flag (see
    // `math_engine`). It is checked before anything else: the worker's stdout carries only its
    // protocol, so nothing may be logged or printed first.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == cli::MATH_WORKER_FLAG)
    {
        return lawh_math::worker_main();
    }
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
/// They are coloured only when stderr is a terminal, so a log file has no escape codes.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
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

    let event_loop = EventLoop::<app::MathReady>::with_user_event()
        .build()
        .context("could not open a window")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let math = math_engine()?;
    // The engine calls this from its own thread when a formula finishes; the window then draws
    // it. The mutex makes the proxy shareable on every platform.
    let proxy = Mutex::new(event_loop.create_proxy());
    math.set_waker(Arc::new(move || {
        let sent = proxy
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(app::MathReady);
        if sent.is_err() {
            tracing::debug!("math finished after the window closed");
        }
    }));

    let renderer = Renderer::with_math(math);
    let mut app = app::App::new(board, renderer, window_title(path), started);
    event_loop.run_app(&mut app).context("the window failed")?;
    app.into_result()
}

/// The math engine for a board from a file: formulas are untrusted, so they compile in a worker
/// process, this executable started with [`cli::MATH_WORKER_FLAG`], which has a memory cap and is
/// killed when a formula takes too long. The worker starts with the first formula.
fn math_engine() -> anyhow::Result<MathEngine> {
    let exe = std::env::current_exe()
        .context("could not find the lawh executable, which also compiles math")?;
    Ok(MathEngine::with_worker(
        WorkerCommand::new(exe).arg(cli::MATH_WORKER_FLAG),
    ))
}

/// Reads and parses a board. Errors read `<path>: <io error>` or `<path>:<line>:<col>: <message>`.
///
/// The file's size is checked before anything is read, so a huge file is refused without being
/// loaded into memory. Reading stops one byte past the limit, in case the file grows meanwhile
/// or reports no size (a pipe).
fn load_board(path: &Path) -> anyhow::Result<Board> {
    let in_path = || path.display().to_string();
    let file = File::open(path).with_context(in_path)?;
    let len = file.metadata().with_context(in_path)?.len();
    let limit = u64::try_from(MAX_FILE_BYTES).unwrap_or(u64::MAX);
    if len > limit {
        let err = FormatError::FileTooLarge {
            bytes: usize::try_from(len).unwrap_or(usize::MAX),
        };
        return Err(anyhow!("{}:{err}", path.display()));
    }
    let mut src = String::new();
    file.take(limit.saturating_add(1))
        .read_to_string(&mut src)
        .with_context(in_path)?;
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
