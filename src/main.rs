use std::{env, error::Error, process::ExitCode, time::Duration};

use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use zbus::{Connection, Proxy, fdo, zvariant::OwnedFd};

const UPOWER_NAME: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_INTERFACE: &str = "org.freedesktop.UPower";
const LOGIND_NAME: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_INTERFACE: &str = "org.freedesktop.login1.Manager";

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Default)]
struct PolicyEngine {
    unsafe_latched: bool,
}

impl PolicyEngine {
    fn update(&mut self, on_battery: bool, lid_closed: bool) -> bool {
        let unsafe_now = on_battery && lid_closed;
        let should_suspend = unsafe_now && !self.unsafe_latched;
        self.unsafe_latched = unsafe_now;
        should_suspend
    }
}

fn relevant_properties<'a>(
    changed: impl Iterator<Item = &'a str>,
    invalidated: impl Iterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut names = changed
        .chain(invalidated)
        .filter(|name| matches!(*name, "OnBattery" | "LidIsClosed"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
}

async fn read_state(upower: &Proxy<'_>) -> zbus::Result<(bool, bool)> {
    let on_battery = upower.get_property("OnBattery").await?;
    let lid_closed = upower.get_property("LidIsClosed").await?;
    Ok((on_battery, lid_closed))
}

async fn evaluate(
    upower: &Proxy<'_>,
    logind: &Proxy<'_>,
    policy: &mut PolicyEngine,
    last_observed: &mut Option<(bool, bool)>,
    reason: &str,
    dry_run: bool,
) -> zbus::Result<()> {
    let observed = read_state(upower).await?;
    if Some(observed) != *last_observed {
        eprintln!(
            "INFO state reason={reason} on_battery={} lid_closed={}",
            observed.0, observed.1
        );
        *last_observed = Some(observed);
    }

    if policy.update(observed.0, observed.1) {
        if dry_run {
            eprintln!("WARN dry-run suspend reason={reason}");
        } else {
            eprintln!("WARN requesting suspend reason={reason}");
            if let Err(error) = logind.call::<_, _, ()>("Suspend", &(false,)).await {
                eprintln!("ERROR suspend request failed: {error}");
            }
        }
    }
    Ok(())
}

async fn run_epoch(dry_run: bool) -> Result<()> {
    let connection = Connection::system().await?;
    let upower = Proxy::new(&connection, UPOWER_NAME, UPOWER_PATH, UPOWER_INTERFACE).await?;
    let logind = Proxy::new(&connection, LOGIND_NAME, LOGIND_PATH, LOGIND_INTERFACE).await?;
    let properties = fdo::PropertiesProxy::builder(&connection)
        .destination(UPOWER_NAME)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let dbus = fdo::DBusProxy::new(&connection).await?;
    let mut property_changes = properties.receive_properties_changed().await?;
    let mut owner_changes = dbus.receive_name_owner_changed().await?;

    let _inhibitor: OwnedFd = logind
        .call(
            "Inhibit",
            &(
                "handle-lid-switch",
                "Omarchy Power Policy",
                "Apply AC/battery lid policy",
                "block",
            ),
        )
        .await?;
    eprintln!("INFO inhibitor acquired what=handle-lid-switch mode=block");

    let mut policy = PolicyEngine::default();
    let mut last_observed = None;
    evaluate(
        &upower,
        &logind,
        &mut policy,
        &mut last_observed,
        "startup",
        dry_run,
    )
    .await?;

    loop {
        tokio::select! {
            change = property_changes.next() => {
                let change = change.ok_or("UPower property signal stream ended")?;
                let args = change.args()?;
                if args.interface_name().as_str() != UPOWER_INTERFACE {
                    continue;
                }
                let relevant = relevant_properties(
                    args.changed_properties().keys().copied(),
                    args.invalidated_properties().iter().copied(),
                );
                if !relevant.is_empty() {
                    let reason = format!("upower:{}", relevant.join(","));
                    evaluate(
                        &upower,
                        &logind,
                        &mut policy,
                        &mut last_observed,
                        &reason,
                        dry_run,
                    ).await?;
                }
            }
            change = owner_changes.next() => {
                let change = change.ok_or("D-Bus owner signal stream ended")?;
                let args = change.args()?;
                let name = args.name().as_str();
                if name == UPOWER_NAME || name == LOGIND_NAME {
                    eprintln!("WARN D-Bus owner changed service={name}; rebuilding policy epoch");
                    return Ok(());
                }
            }
        }
    }
}

fn parse_dry_run() -> std::result::Result<bool, &'static str> {
    let mut dry_run = false;
    for argument in env::args().skip(1) {
        match argument.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "--help" => {
                println!("Usage: omarchy-power-policy [--dry-run]");
                return Err("");
            }
            _ => return Err("unknown or duplicate argument"),
        }
    }
    Ok(dry_run)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let dry_run = match parse_dry_run() {
        Ok(value) => value,
        Err("") => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ERROR {error}");
            return ExitCode::from(2);
        }
    };

    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("ERROR cannot install SIGTERM handler: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("ERROR cannot install SIGINT handler: {error}");
            return ExitCode::FAILURE;
        }
    };

    loop {
        tokio::select! {
            _ = terminate.recv() => {
                eprintln!("INFO shutting down signal=SIGTERM");
                return ExitCode::SUCCESS;
            }
            _ = interrupt.recv() => {
                eprintln!("INFO shutting down signal=SIGINT");
                return ExitCode::SUCCESS;
            }
            result = run_epoch(dry_run) => {
                if let Err(error) = result {
                    eprintln!("ERROR policy epoch failed: {error}");
                }
            }
        }

        tokio::select! {
            _ = terminate.recv() => return ExitCode::SUCCESS,
            _ = interrupt.recv() => return ExitCode::SUCCESS,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PolicyEngine, relevant_properties};

    #[test]
    fn close_on_ac_then_unplug_suspends_once() {
        let mut policy = PolicyEngine::default();
        assert!(!policy.update(false, false));
        assert!(!policy.update(false, true));
        assert!(policy.update(true, true));
        assert!(!policy.update(true, true));
    }

    #[test]
    fn close_while_on_battery_suspends() {
        let mut policy = PolicyEngine::default();
        assert!(!policy.update(true, false));
        assert!(policy.update(true, true));
    }

    #[test]
    fn opening_lid_rearms_policy() {
        let mut policy = PolicyEngine::default();
        assert!(policy.update(true, true));
        assert!(!policy.update(true, false));
        assert!(policy.update(true, true));
    }

    #[test]
    fn plugging_ac_rearms_closed_lid() {
        let mut policy = PolicyEngine::default();
        assert!(policy.update(true, true));
        assert!(!policy.update(false, true));
        assert!(policy.update(true, true));
    }

    #[test]
    fn open_lid_unplug_does_nothing() {
        let mut policy = PolicyEngine::default();
        assert!(!policy.update(false, false));
        assert!(!policy.update(true, false));
    }

    #[test]
    fn filters_relevant_property_names() {
        let changed = ["DaemonVersion", "OnBattery", "LidIsClosed"];
        let invalidated = ["LidIsClosed"];
        assert_eq!(
            relevant_properties(changed.into_iter(), invalidated.into_iter()),
            ["LidIsClosed", "OnBattery"]
        );
    }
}
