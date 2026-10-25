//! The lawh app: opens a `.lawh` board in a window to edit with Vim keys, with mouse pan, zoom
//! and select. `lawh export` draws a board into an SVG, PNG or PDF file without a window. With
//! `--math-worker` it runs as the math worker instead.

mod app;
mod cli;
mod export;
mod files;
mod keys;
mod rc;
mod status;
#[cfg(test)]
mod test_dir;
mod view;

use std::io::IsTerminal as _;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::Context as _;
use lawh_core::Engine;
use lawh_core::engine::Symbol;
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
    // protocol, so nothing may be printed there. Its logs go to stderr, which it shares with
    // the app.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == cli::MATH_WORKER_FLAG)
    {
        init_logging();
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
            if let Err(err) = run(path.as_deref(), started) {
                cli::print_error(&err);
                return ExitCode::FAILURE;
            }
        }
        Command::Export(args) => match export::run(&args) {
            // Nothing on stdout: a note (failed formulas) goes to stderr, and the exit is 0.
            Ok(note) => {
                if let Some(note) = note {
                    cli::print_note(&note);
                }
            }
            Err(err) => {
                cli::print_error(&err);
                return ExitCode::FAILURE;
            }
        },
    }
    ExitCode::SUCCESS
}

/// Logs go to stderr, filtered by `RUST_LOG` (default `warn`), for example `RUST_LOG=debug lawh`.
/// They are coloured only when stderr is a terminal, so a log file has no escape codes. Never
/// stdout: the math worker's stdout carries its protocol.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
}

/// Opens the board in `path` (see [`files::plan_open`]) in a window and runs until the window is
/// closed or the engine quits.
fn run(path: Option<&Path>, started: Instant) -> anyhow::Result<()> {
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "lawh starting");
    // Read the board first, so a bad file is reported without opening a window.
    let opened = files::plan_open(path)?;
    tracing::info!(
        path = ?opened.path,
        new_file = opened.new_file,
        objects = opened.board.len(),
        "board loaded"
    );
    let mut engine =
        Engine::new(opened.board).with_file_name(opened.path.as_deref().map(files::engine_name));
    // Made here, not in `main`: the math worker never needs the list.
    let symbols_started = Instant::now();
    let kept = engine.set_symbols(drawable_symbols());
    tracing::info!(
        kept,
        ms = symbols_started.elapsed().as_secs_f64() * 1000.0,
        "symbols ready for :sym"
    );
    // The lawhrc is read before the first key, as Vim reads its vimrc.
    let message = rc::path().and_then(|rc| rc::source(&mut engine, &rc));
    let launch_dir = std::env::current_dir()
        .inspect_err(|err| tracing::warn!(%err, "no current directory; :w paths stay relative"))
        .ok();

    let mut builder = EventLoop::<app::MathReady>::with_user_event();
    // macOS's default menu sends Cmd+Q to its Quit item, which quits at once: the window gets
    // no close request, so unwritten changes would be lost without a warning. Without the menu
    // the key reaches the app, which takes it as a close request (`keys::is_close_request`).
    // The menu's other items (Hide, Hide Others, Show All) go with it.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS as _;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build().context("could not open a window")?;
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
    let mut app = app::App::new(engine, opened.path, launch_dir, renderer, started, message);
    event_loop.run_app(&mut app).context("the window failed")?;
    app.into_result()
}

/// The symbols `:sym` picks from: Typst's names that the bundled fonts draw
/// ([`lawh_math::symbols`]), in the order the engine keeps a list in, so it takes this one
/// without a copy ([`Engine::set_symbols`]).
fn drawable_symbols() -> Arc<[Symbol]> {
    lawh_math::symbols()
        .into_iter()
        .filter(|symbol| symbol.drawable)
        .map(|symbol| Symbol {
            name: symbol.name,
            value: symbol.value,
        })
        .collect()
}

/// The math engine for a board from a file, in the window and in `lawh export`: formulas are
/// untrusted, so they compile in a worker process, this executable started with
/// [`cli::MATH_WORKER_FLAG`], which has a memory cap and is killed when a formula takes too long.
/// The worker starts with the first formula.
fn math_engine() -> anyhow::Result<MathEngine> {
    let exe = std::env::current_exe()
        .context("could not find the lawh executable, which also compiles math")?;
    Ok(MathEngine::with_worker(
        WorkerCommand::new(exe).arg(cli::MATH_WORKER_FLAG),
    ))
}

#[cfg(test)]
mod tests {
    use lawh_core::Board;

    use super::*;

    #[test]
    fn the_engine_keeps_every_drawable_symbol() {
        let all = lawh_math::symbols();
        let symbols = drawable_symbols();
        assert_eq!(symbols.len(), all.iter().filter(|s| s.drawable).count());
        assert!(symbols.len() < all.len());
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "alpha" && s.value == "\u{3b1}")
        );
        let mut engine = Engine::new(Board::new());
        assert_eq!(engine.set_symbols(Arc::clone(&symbols)), symbols.len());
    }
}
