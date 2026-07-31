//! Continuous BLE discovery with a watchdog that restarts a wedged scan.

use crate::config::Config;
use crate::decoder;
use crate::exec::Hook;
use crate::metrics::Registry;
use crate::output::{Format, Reading};
use bluer::{Adapter, AdapterEvent, Address, DeviceProperty, Result};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use uuid::Uuid;

/// How often the watchdog checks whether readings are still arriving.
const WATCHDOG_TICK: Duration = Duration::from_secs(5);
/// Settle time before discovery is started again.
const RESTART_DELAY: Duration = Duration::from_secs(2);

/// The service data of the advertisement last seen from a device.
type ServiceData = HashMap<Uuid, Vec<u8>>;

pub struct Settings {
    /// Restart discovery when no reading arrives for this long.
    pub watchdog: Duration,
    /// Pause between restarts.
    pub cooldown: Duration,
    /// How to write readings.
    pub format: Format,
    /// Which sensors to report, and how to name and calibrate them.
    pub config: Config,
    /// External program to run per reading, if any.
    pub exec: Option<Hook>,
    /// Where readings are accumulated for Prometheus, if enabled.
    pub metrics: Option<Arc<Registry>>,
}

/// Scan until the adapter disappears or the task is cancelled.
pub async fn run(adapter: &Adapter, settings: Settings) -> Result<()> {
    // The advertisement each device last sent, so that a property change which
    // carries no new reading (an RSSI update, say) is not reported twice.
    let mut last_service_data: HashMap<Address, ServiceData> = HashMap::new();
    let last_reading_at = Arc::new(Mutex::new(Instant::now()));
    let (tx, mut rx) = mpsc::unbounded_channel::<AdapterEvent>();

    spawn_discovery(
        adapter.clone(),
        tx,
        last_reading_at.clone(),
        settings.watchdog,
        settings.cooldown,
    );

    while let Some(event) = rx.recv().await {
        match event {
            AdapterEvent::DeviceAdded(addr) => {
                // Checked before reading any properties, so a filtered-out device
                // costs nothing but the event itself.
                if !settings.config.accepts_address(addr) {
                    continue;
                }

                if let Err(e) = handle_device(
                    adapter,
                    addr,
                    &mut last_service_data,
                    &last_reading_at,
                    &settings,
                )
                .await
                {
                    log::warn!("error handling device {addr}: {e}");
                }
            }
            AdapterEvent::DeviceRemoved(addr) => {
                log::debug!("device removed: {addr}");
                last_service_data.remove(&addr);
            }
            _ => {}
        }
    }

    Ok(())
}

/// Run discovery in the background, forwarding events and restarting the scan if
/// it goes quiet.
fn spawn_discovery(
    adapter: Adapter,
    tx: mpsc::UnboundedSender<AdapterEvent>,
    last_reading_at: Arc<Mutex<Instant>>,
    watchdog: Duration,
    cooldown: Duration,
) {
    tokio::spawn(async move {
        let mut restarts: u64 = 0;

        loop {
            log::info!("starting discovery");

            // discover_devices_with_changes() re-emits DeviceAdded every time a
            // device's properties change, which is what makes this a continuous
            // reader rather than a one-shot inventory.
            let mut events = match adapter.discover_devices_with_changes().await {
                Ok(events) => events,
                Err(e) => {
                    log::error!("failed to start discovery: {e}");
                    sleep(cooldown).await;
                    continue;
                }
            };

            loop {
                tokio::select! {
                    event = events.next() => {
                        match event {
                            Some(event) => {
                                if tx.send(event).is_err() {
                                    // The reader is gone, so there is nobody left
                                    // to scan for.
                                    return;
                                }
                            }
                            None => {
                                log::warn!("discovery stream ended, restarting");
                                break;
                            }
                        }
                    }

                    _ = sleep(WATCHDOG_TICK) => {
                        let idle = last_reading_at.lock().await.elapsed();
                        if idle > watchdog {
                            restarts += 1;
                            log::warn!(
                                "watchdog: no readings for {idle:?}, restarting discovery \
                                 (restart {restarts})"
                            );

                            // Dropping the stream is the equivalent of
                            // disable_le_scan.
                            drop(events);
                            sleep(cooldown).await;
                            break;
                        }
                    }
                }
            }

            sleep(RESTART_DELAY).await;
        }
    });
}

async fn handle_device(
    adapter: &Adapter,
    addr: Address,
    last_service_data: &mut HashMap<Address, ServiceData>,
    last_reading_at: &Mutex<Instant>,
    settings: &Settings,
) -> Result<()> {
    let device = adapter.device(addr)?;

    // One D-Bus round-trip for every property, instead of one round-trip each for
    // the name, the RSSI and the service data. At one advertisement per second
    // per sensor that is the difference the Pi Zero W notices.
    let mut name: Option<String> = None;
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

    if !settings.config.accepts_rssi(rssi) {
        log::trace!("{addr}: ignored, RSSI {rssi:?} is below the configured minimum");
        return Ok(());
    }

    // Identical to the advertisement we already reported, so some other property
    // changed. Sensors bump a packet counter on every broadcast, so a genuinely
    // new reading always differs here.
    if last_service_data.get(&addr) == Some(&service_data) {
        return Ok(());
    }

    if log::log_enabled!(log::Level::Trace) {
        for (uuid, bytes) in &service_data {
            log::trace!("{addr} service {uuid}: {bytes:02X?}");
        }
    }

    if let Some(mut data) = decoder::handle_service_data(&service_data) {
        // A configured name beats whatever the device calls itself, and the
        // calibration offsets are applied before anything sees the reading.
        if let Some(sensor) = settings.config.settings(addr) {
            sensor.calibrate(&mut data);
            if sensor.name.is_some() {
                name = sensor.name.clone();
            }
        }

        let reading = Reading::new(addr, name, rssi, &data);
        reading.emit(settings.format);

        if let Some(registry) = &settings.metrics {
            registry.record(&reading);
        }

        if let Some(hook) = &settings.exec {
            hook.run(&reading);
        }

        // Only an actual reading counts as proof that the scan is alive.
        *last_reading_at.lock().await = Instant::now();
    }

    last_service_data.insert(addr, service_data);

    Ok(())
}
