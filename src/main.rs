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

/// Simple BLE discovery tool with watchdog restart (Python-style)
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Watchdog timeout in seconds (restart if no packets seen)
    #[arg(long, default_value_t = 20)]
    watchdog: u64,

    /// Cooldown pause between restarts in seconds
    #[arg(long, default_value_t = 5)]
    cooldown: u64,
}

/// The service data of the advertisement last seen from a device.
type ServiceData = HashMap<Uuid, Vec<u8>>;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    println!(
        "Starting robust continuous BLE discovery (watchdog={}s, cooldown={}s)...",
        args.watchdog, args.cooldown
    );

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

                // Small delay before reinitializing discovery
                sleep(Duration::from_secs(2)).await;
            }
        });
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
            _ => {}
        }
    }

    Ok(())
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
