use std::time::Duration;
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use tokio::time;
mod decoder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Starting BLE scan...");

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;

    if adapters.is_empty() {
        eprintln!("No BLE adapters found");
        return Ok(());
    }

    let adapter = &adapters[0];

    adapter.start_scan(ScanFilter::default()).await?;
    println!("Scanning for BLE devices...");

    loop {
        let peripherals = adapter.peripherals().await?;
        for p in peripherals {
            let properties = p.properties().await?;
            let address = p.address();
            if let Some(props) = properties {
                let name = props.local_name.clone().unwrap_or_default();
                println!("📡 Found device: {}, name={}", address, name);
                decoder::classify_and_decode(&props);                
            }
        }

        time::sleep(Duration::from_secs(3)).await;
    }
}