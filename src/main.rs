use clap::Parser;
use log::LevelFilter;
use std::path::PathBuf;
use std::time::Duration;

mod config;
mod decoder;
mod exec;
mod output;
mod scan;

use config::Config;
use exec::Hook;
use output::Format;

/// Read environmental data from Bluetooth sensors.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Configuration file naming and calibrating known sensors
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Ignore sensors that have no entry in the configuration file
    #[arg(long)]
    only_known: bool,

    /// Ignore advertisements weaker than this, in dBm (e.g. -90)
    #[arg(long, value_name = "DBM", allow_negative_numbers = true)]
    min_rssi: Option<i16>,

    /// Run this program once per reading, with the reading in MITEMPR_*
    /// environment variables and as JSON on its standard input
    #[arg(long, value_name = "PATH")]
    exec: Option<PathBuf>,

    /// Shortest gap in seconds between two --exec runs for the same sensor
    #[arg(long, value_name = "SECS", default_value_t = 0, requires = "exec")]
    exec_interval: u64,

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

    /// Read the configuration file, if one was given, and let the command line
    /// override what it says.
    fn load_config(&self) -> Result<Config, config::ConfigError> {
        let mut config = match &self.config {
            Some(path) => Config::load(path)?,
            None => Config::default(),
        };

        if self.only_known {
            config.set_only_known();
        }
        if let Some(min_rssi) = self.min_rssi {
            config.set_min_rssi(min_rssi);
        }

        Ok(config)
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // RUST_LOG still wins, so a systemd unit can turn on debug logging without
    // its command line changing.
    env_logger::Builder::new()
        .filter_level(args.log_level())
        .parse_env("RUST_LOG")
        .init();

    let config = args.load_config()?;

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    log::info!(
        "scanning on {} ({} configured sensor(s){}, watchdog {}s, cooldown {}s)",
        adapter.name(),
        config.configured_sensors(),
        if config.only_known() {
            ", ignoring the rest"
        } else {
            ""
        },
        args.watchdog,
        args.cooldown
    );

    let settings = scan::Settings {
        watchdog: Duration::from_secs(args.watchdog),
        cooldown: Duration::from_secs(args.cooldown),
        format: args.format,
        config,
        exec: args
            .exec
            .clone()
            .map(|program| Hook::new(program, Duration::from_secs(args.exec_interval))),
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
