use clap::Parser;
use log::LevelFilter;
use std::time::Duration;

mod decoder;
mod output;
mod scan;

use output::Format;

/// Read environmental data from Bluetooth sensors.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Restart discovery if no reading arrives for this many seconds
    #[arg(long, default_value_t = 20)]
    watchdog: u64,

    /// Cooldown pause between discovery restarts in seconds
    #[arg(long, default_value_t = 5)]
    cooldown: u64,

    /// How to write readings: a human-readable line, or one JSON object per line
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Log more; repeat for even more (-v debug, -vv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Log only warnings and errors. Readings written with --format json are not
    /// affected, since those go to stdout rather than through the logger.
    #[arg(short, long, conflicts_with = "verbose")]
    quiet: bool,
}

impl Args {
    fn log_level(&self) -> LevelFilter {
        if self.quiet {
            return LevelFilter::Warn;
        }
        match self.verbose {
            0 => LevelFilter::Info,
            1 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> bluer::Result<()> {
    let args = Args::parse();

    // RUST_LOG still wins, so a systemd unit can turn on debug logging without
    // its command line changing.
    env_logger::Builder::new()
        .filter_level(args.log_level())
        .parse_env("RUST_LOG")
        .init();

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    log::info!(
        "scanning on {} (watchdog {}s, cooldown {}s)",
        adapter.name(),
        args.watchdog,
        args.cooldown
    );

    let settings = scan::Settings {
        watchdog: Duration::from_secs(args.watchdog),
        cooldown: Duration::from_secs(args.cooldown),
        format: args.format,
    };

    tokio::select! {
        result = scan::run(&adapter, settings) => result?,
        () = shutdown_requested() => log::info!("shutting down"),
    }

    Ok(())
}

/// Resolves once the process is asked to stop, so discovery is torn down cleanly
/// instead of being killed mid-scan. SIGTERM matters here because that is what
/// systemd sends.
async fn shutdown_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    () = wait_for_ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(e) => {
                log::warn!("cannot listen for SIGTERM: {e}");
                wait_for_ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    wait_for_ctrl_c().await;
}

async fn wait_for_ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        log::warn!("cannot listen for Ctrl-C: {e}");
        // Never resolve, so the scan keeps running instead of exiting at once.
        std::future::pending::<()>().await;
    }
}
