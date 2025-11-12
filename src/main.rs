use bluer::{Adapter, AdapterEvent, Address, Result};
use clap::Parser;
use futures::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
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

    let seen_devices = Arc::new(Mutex::new(HashSet::<Address>::new()));
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
                let mut events = match adapter.discover_devices().await {
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
                                    // ❌ no timestamp update here anymore
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
                                    "⏱ Watchdog: no BLE packets for {:?}, restarting discovery (count {})...",
                                    elapsed, restart_counter
                                );
                                restart_counter += 1;

                                // Drop the current stream (equivalent to disable_le_scan)
                                drop(events);

                                // Wait before restarting (equivalent to Python’s 5s delay)
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
                let mut seen = seen_devices.lock().await;
                if !seen.contains(&addr) {
                    seen.insert(addr);
                    if let Err(e) = handle_device(&adapter, addr, last_ble_packet.clone()).await {
                        eprintln!("Error handling device {addr}: {e}");
                    }
                }
            }
            AdapterEvent::DeviceRemoved(addr) => {
                println!("❌ Device removed: {addr}");
                let mut seen = seen_devices.lock().await;
                seen.remove(&addr);
            }
            _ => {}
        }
    }

    Ok(())
}

async fn handle_device(
    adapter: &Adapter,
    addr: Address,
    last_ble_packet: Arc<Mutex<Instant>>,
) -> Result<()> {
    let device = adapter.device(addr)?;
    let name = device.name().await?.unwrap_or_else(|| "<unknown>".into());
    let rssi = device.rssi().await?.unwrap_or(0);

    println!("📡 {addr} ({name}), RSSI={rssi}");

    if let Some(data_map) = device.service_data().await? {
        for (uuid, data) in &data_map {
            println!("  Service {uuid}: {:02X?}", data);
        }

        if let Some(decoded) = decoder::handle_service_data(&data_map) {
            println!("  🔍 Got sensor reading: {:?}", decoded);

            // ✅ Reset watchdog timer only on actual service data
            *last_ble_packet.lock().await = Instant::now();
        }
    }

    // Uncomment this if you also want manufacturer data
    /*
    if let Some(mdata) = device.manufacturer_data().await? {
        for (id, data) in mdata {
            println!("  Manufacturer {id:#06X}: {:02X?}", data);
        }
    }
    */

    Ok(())
}
