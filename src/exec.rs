//! Handing each reading to an external program.

use crate::output::Reading;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

/// How many hook processes may be alive at once. A Pi Zero W has one core and
/// 512 MB of RAM, and several sensors advertise every second or two, so an
/// unbounded fan-out of shell scripts is a real way to bring the machine down.
const MAX_PARALLEL: usize = 4;

/// A hook that has not finished by then is assumed to be stuck and is killed,
/// so it cannot hold one of the slots above forever.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Runs an external program once per reading.
pub struct Hook {
    program: PathBuf,
    /// Shortest gap between two runs for the same sensor.
    min_interval: Duration,
    slots: Arc<Semaphore>,
    /// When the hook last ran, per sensor address.
    last_run: Mutex<HashMap<String, Instant>>,
}

impl Hook {
    pub fn new(program: PathBuf, min_interval: Duration) -> Self {
        Self {
            program,
            min_interval,
            slots: Arc::new(Semaphore::new(MAX_PARALLEL)),
            last_run: Mutex::new(HashMap::new()),
        }
    }

    /// Whether enough time has passed since this sensor's last hook run, marking
    /// it as run if so.
    fn claim(&self, address: &str) -> bool {
        let now = Instant::now();
        let mut last_run = self
            .last_run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(previous) = last_run.get(address)
            && now.duration_since(*previous) < self.min_interval
        {
            return false;
        }

        last_run.insert(address.to_owned(), now);
        true
    }

    /// Run the hook for this reading.
    ///
    /// Returns immediately: the child runs in its own task so that a slow script
    /// cannot stall the scan. Readings are dropped rather than queued when the
    /// hook cannot keep up, since a queue of stale temperatures is worse than a
    /// gap.
    pub fn run(&self, reading: &Reading) {
        if !self.claim(&reading.address) {
            log::trace!(
                "exec: skipping {}, less than {:?} since the last run",
                reading.address,
                self.min_interval
            );
            return;
        }

        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            log::warn!(
                "exec: {} hooks already running, dropping the reading from {}",
                MAX_PARALLEL,
                reading.address
            );
            return;
        };

        let program = self.program.clone();
        let environment = environment(reading);
        let json = match serde_json::to_string(reading) {
            Ok(json) => json,
            Err(e) => {
                log::error!("exec: could not serialise reading: {e}");
                return;
            }
        };

        tokio::spawn(async move {
            // Held for as long as the child lives.
            let _permit = permit;

            match tokio::time::timeout(TIMEOUT, invoke(&program, &environment, &json)).await {
                Ok(Ok(status)) if status.success() => {
                    log::debug!("exec: {} finished", program.display());
                }
                Ok(Ok(status)) => {
                    log::warn!("exec: {} exited with {status}", program.display());
                }
                Ok(Err(e)) => log::warn!("exec: {} failed: {e}", program.display()),
                Err(_) => log::warn!(
                    "exec: {} did not finish within {TIMEOUT:?}, killed",
                    program.display()
                ),
            }
        });
    }
}

async fn invoke(
    program: &Path,
    environment: &[(&'static str, String)],
    json: &str,
) -> std::io::Result<std::process::ExitStatus> {
    let mut child = Command::new(program)
        .envs(environment.iter().map(|(name, value)| (*name, value)))
        .stdin(Stdio::piped())
        // Deliberately not inherited: a chatty script would otherwise interleave
        // itself with --format json output and corrupt the stream. stderr is
        // inherited so a broken script still says so.
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        // Dropping the pipe closes it, so a script reading to EOF is not left
        // waiting.
    }

    child.wait().await
}

/// The environment handed to the hook.
///
/// Every variable is always set, empty when the sensor did not report that
/// measurement. Leaving them out instead would mean a value inherited from
/// mitempr's own environment could show through.
fn environment(reading: &Reading) -> Vec<(&'static str, String)> {
    fn number<T: std::fmt::Display>(value: Option<T>) -> String {
        value.map(|value| value.to_string()).unwrap_or_default()
    }

    vec![
        ("MITEMPR_MAC", reading.address.clone()),
        ("MITEMPR_NAME", reading.name.clone().unwrap_or_default()),
        ("MITEMPR_FORMAT", reading.format.to_owned()),
        ("MITEMPR_TIMESTAMP", reading.timestamp.to_string()),
        ("MITEMPR_RSSI", number(reading.rssi_dbm)),
        ("MITEMPR_TEMPERATURE", number(reading.temperature_celsius)),
        ("MITEMPR_HUMIDITY", number(reading.humidity_percent)),
        ("MITEMPR_BATTERY", number(reading.battery_percent)),
        ("MITEMPR_VOLTAGE", number(reading.voltage_volts)),
        ("MITEMPR_PRESSURE", number(reading.pressure_hpa)),
        ("MITEMPR_ILLUMINANCE", number(reading.illuminance_lux)),
        ("MITEMPR_MOISTURE", number(reading.moisture_percent)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{SensorData, SensorFormat};
    use bluer::Address;

    fn reading() -> Reading {
        let data = SensorData {
            format: SensorFormat::Pvvx,
            temperature: Some(22.9),
            humidity: Some(64.25),
            battery: Some(16),
            ..Default::default()
        };
        let mut reading = Reading::new(
            Address::new([0xA4, 0xC1, 0x38, 0xA0, 0x7B, 0x03]),
            Some("Living Room".to_string()),
            Some(-67),
            &data,
        );
        reading.timestamp = 1_770_000_000;
        reading
    }

    fn variable(environment: &[(&'static str, String)], key: &str) -> String {
        environment
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("{key} should be present"))
    }

    #[test]
    fn every_measurement_becomes_a_variable() {
        let environment = environment(&reading());

        assert_eq!(variable(&environment, "MITEMPR_MAC"), "A4:C1:38:A0:7B:03");
        assert_eq!(variable(&environment, "MITEMPR_NAME"), "Living Room");
        assert_eq!(variable(&environment, "MITEMPR_FORMAT"), "pvvx");
        assert_eq!(variable(&environment, "MITEMPR_TIMESTAMP"), "1770000000");
        assert_eq!(variable(&environment, "MITEMPR_RSSI"), "-67");
        assert_eq!(variable(&environment, "MITEMPR_TEMPERATURE"), "22.9");
        assert_eq!(variable(&environment, "MITEMPR_HUMIDITY"), "64.25");
        assert_eq!(variable(&environment, "MITEMPR_BATTERY"), "16");
    }

    /// A measurement the sensor did not send must be present but empty, so a
    /// script can test for it and cannot pick up a stale inherited value.
    #[test]
    fn absent_measurements_become_empty_variables() {
        let environment = environment(&reading());

        assert_eq!(variable(&environment, "MITEMPR_PRESSURE"), "");
        assert_eq!(variable(&environment, "MITEMPR_ILLUMINANCE"), "");
        assert_eq!(variable(&environment, "MITEMPR_MOISTURE"), "");
        assert_eq!(variable(&environment, "MITEMPR_VOLTAGE"), "");
    }

    #[test]
    fn a_zero_interval_never_holds_a_reading_back() {
        let hook = Hook::new(PathBuf::from("/bin/true"), Duration::ZERO);

        assert!(hook.claim("A4:C1:38:A0:7B:03"));
        assert!(hook.claim("A4:C1:38:A0:7B:03"));
    }

    #[test]
    fn the_interval_is_tracked_per_sensor() {
        let hook = Hook::new(PathBuf::from("/bin/true"), Duration::from_secs(60));

        assert!(hook.claim("A4:C1:38:A0:7B:03"));
        assert!(
            !hook.claim("A4:C1:38:A0:7B:03"),
            "the same sensor is held back"
        );
        assert!(hook.claim("A4:C1:38:AA:BB:CC"), "a different sensor is not");
    }

    /// The end-to-end path: a real script, the environment and the JSON on
    /// stdin.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_hook_receives_the_environment_and_the_json() {
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("mitempr-exec-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp directory should be creatable");
        let script = directory.join("hook.sh");
        let output = directory.join("output");

        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nread -r line\n{{ echo \"$MITEMPR_NAME|$MITEMPR_TEMPERATURE|$MITEMPR_MOISTURE\"; echo \"$line\"; }} > {}\n",
                output.display()
            ),
        )
        .expect("script should be writable");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script should be executable");

        let reading = reading();
        let status = invoke(
            &script,
            &environment(&reading),
            &serde_json::to_string(&reading).expect("reading should serialise"),
        )
        .await
        .expect("hook should run");
        assert!(status.success(), "hook exited with {status}");

        let written =
            std::fs::read_to_string(&output).expect("hook should have written its output");
        let mut lines = written.lines();
        assert_eq!(lines.next(), Some("Living Room|22.9|"));
        assert_eq!(
            lines.next(),
            Some(
                serde_json::to_string(&reading)
                    .expect("reading should serialise")
                    .as_str()
            )
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
