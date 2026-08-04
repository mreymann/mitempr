use bluer::{Adapter, AdapterEvent, Address, DeviceProperty, Result};
use clap::Parser;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use uuid::Uuid;
mod decoder;
mod output;
mod scan;

use config::Config;
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

/// The service data of the advertisement last seen from a device.
type ServiceData = HashMap<Uuid, Vec<u8>>;

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

    // The advertisement each device last sent, so that a property change which
    // carries no new reading (an RSSI update, say) is not reported twice.
    let mut last_service_data: HashMap<Address, ServiceData> = HashMap::new();
    let last_ble_packet = Arc::new(Mutex::new(Instant::now()));
    let (tx, mut rx) = mpsc::unbounded_channel::<AdapterEvent>();

    //
    // 🔄 Discovery + watchdog task
    //
    {
        let adapter = adapter.clone();
        let tx = tx.clone();
        let last_ble_packet = last_ble_packet.clone();
        let watchdog = args.watchdog;
        let cooldown = args.cooldown;

        tokio::spawn(async move {
            let mut restart_counter: u64 = 1;

            loop {
                println!("🔍 (Re)starting discovery...");

                // discover_devices_with_changes() re-emits DeviceAdded every time
                // a device's properties change, which is what turns this into a
                // continuous reader: discover_devices() only reports each device
                // once, so every advertisement after the first was invisible.
                let mut events = match adapter.discover_devices_with_changes().await {
                    Ok(ev) => ev,
                    Err(e) => {
                        eprintln!("❌ Failed to start discovery: {e}");
                        sleep(Duration::from_secs(cooldown)).await;
                        continue;
                    }
                };

                loop {
                    tokio::select! {
                        evt = events.next() => {
                            match evt {
                                Some(AdapterEvent::DeviceAdded(addr)) => {
                                    let _ = tx.send(AdapterEvent::DeviceAdded(addr));
                                }
                                Some(AdapterEvent::DeviceRemoved(addr)) => {
                                    let _ = tx.send(AdapterEvent::DeviceRemoved(addr));
                                }
                                Some(_) => {}
                                None => {
                                    println!("⚠️ Discovery stream ended — restarting...");
                                    break;
                                }
                            }
                        }

                        _ = sleep(Duration::from_secs(5)) => {
                            let elapsed = last_ble_packet.lock().await.elapsed();
                            if elapsed > Duration::from_secs(watchdog) {
                                println!(
                                    "⏱ Watchdog: no BLE packets for {elapsed:?}, restarting discovery (count {restart_counter})..."
                                );
                                restart_counter += 1;

                                // Drop the current stream (equivalent to disable_le_scan)
                                drop(events);

                                // Wait before restarting (equivalent to Python's 5s delay)
                                sleep(Duration::from_secs(cooldown)).await;

                                break;
                            }
                        }
                    }
                }

    tokio::select! {
        result = scan::run(&adapter, settings) => result?,
        () = shutdown_requested() => log::info!("shutting down"),
    }

    //
    // 📡 Event processing loop
    //
    while let Some(evt) = rx.recv().await {
        match evt {
            AdapterEvent::DeviceAdded(addr) => {
                if let Err(e) =
                    handle_device(&adapter, addr, &mut last_service_data, &last_ble_packet).await
                {
                    eprintln!("Error handling device {addr}: {e}");
                }
            }
            AdapterEvent::DeviceRemoved(addr) => {
                last_service_data.remove(&addr);
            }
        }
    }

    #[cfg(not(unix))]
    wait_for_ctrl_c().await;
}

async fn handle_device(
    adapter: &Adapter,
    addr: Address,
    last_service_data: &mut HashMap<Address, ServiceData>,
    last_ble_packet: &Mutex<Instant>,
) -> Result<()> {
    let device = adapter.device(addr)?;

    // One D-Bus round-trip for every property, instead of one round-trip each for
    // the name, the RSSI and the service data. At one advertisement per second
    // per sensor that is the difference the Pi Zero W notices.
    let mut name = None;
    let mut rssi = None;
    let mut service_data = None;
    for property in device.all_properties().await? {
        match property {
            DeviceProperty::Name(value) => name = Some(value),
            DeviceProperty::Rssi(value) => rssi = Some(value),
            DeviceProperty::ServiceData(value) => service_data = Some(value),
            _ => {}
        }
    }

    // No service data at all: not something this tool can read.
    let Some(service_data) = service_data else {
        return Ok(());
    };

    // Identical to the advertisement we already reported, so some other property
    // changed. Sensors bump a packet counter on every broadcast, so a genuinely
    // new reading always differs here.
    if last_service_data.get(&addr) == Some(&service_data) {
        return Ok(());
    }

    if let Some(decoded) = decoder::handle_service_data(&service_data) {
        let name = name.as_deref().unwrap_or("<unknown>");
        let rssi = rssi.map_or_else(|| "n/a".to_string(), |value| value.to_string());
        println!("📡 {addr} ({name}), RSSI={rssi}");
        println!("  🔍 Got sensor reading: {decoded:?}");

        // ✅ Reset watchdog timer only on actual sensor readings
        *last_ble_packet.lock().await = Instant::now();
    }

    last_service_data.insert(addr, service_data);

    Ok(())
}
