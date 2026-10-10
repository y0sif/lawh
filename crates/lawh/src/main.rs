//! The lawh app: window, input, status line and command line.

use tracing_subscriber::EnvFilter;

fn main() {
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "lawh starting");
    print_version();
}

/// Logs go to stderr, filtered by `RUST_LOG` (default `warn`), for example `RUST_LOG=debug lawh`.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[expect(
    clippy::print_stdout,
    reason = "placeholder output until the window exists"
)]
fn print_version() {
    println!("lawh {}", env!("CARGO_PKG_VERSION"));
}
