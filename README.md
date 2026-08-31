# Omarchy Power Policy

A focused Omarchy service plugin for one policy:

| State transition | Action |
|---|---|
| Lid closes while on AC | Keep running |
| Lid closes while on battery | Suspend |
| AC is unplugged while the lid is closed | Suspend |
| AC is unplugged while the lid is open | No action |

External-monitor state does not override this policy. Omarchy continues to own clamshell display handling, idle/screensaver behavior, and its existing `sleep:delay` lock-before-suspend path.

## Current policy rationale

Keeping a lid-closed laptop running on AC is deliberate. The laptop may be set aside with its lid closed while long-running jobs, downloads, backups, builds, or services continue. An external display going to sleep, being powered off, or disconnecting must not turn that workload into a suspend request.

Consequently:

- closing the lid on AC always keeps the system running, whether an external display is connected, active, asleep, powered off, or disconnected;
- external-display state changes while the lid remains closed never request suspend;
- unplugging AC while the lid is closed requests suspend because the machine has transitioned to the battery policy.

This intentionally overrides logind's default docked/undocked lid handling. The daemon holds the low-level `handle-lid-switch:block` inhibitor so losing an external display cannot cause logind to suspend behind this policy.

## Overall power-management model

The overall design has two top-level modes selected by the current power source: **AC** and **battery**.

### AC mode: server mode

AC mode treats the laptop as a server that may be running unattended workloads. It does not suspend automatically by default. Automatic suspend occurs only when the user explicitly configures an AC suspend policy.

This means that closing the lid, an external display going to sleep, or an external display disconnecting does not by itself suspend the machine in the default AC mode.

### Battery mode

Battery mode has two logical activity states:

- **active**: the laptop works normally;
- **inactive**: the policy requests suspend.

Battery mode can enter the inactive state through either of these triggers:

1. **Lid closed.** In the current implementation, closing the lid on battery enters inactive unconditionally and requests suspend.
2. **Idle timeout.** A sufficiently long period without user activity may enter inactive. This trigger must honor legitimate idle inhibitors such as Caffeine, Omarchy Stay Awake, media playback, presentations, or applications that explicitly inhibit idle handling.

A future option may refine the lid trigger so that closing the lid enters inactive only when the lid is closed **and no external display is working**. The exact definition of a “working” external display—merely connected, enabled, actively presenting, or DPMS-on—must be specified before implementing that option rather than inferred from cable presence.

The current daemon implements the AC/battery source transition and lid-triggered subset of this model. Battery idle timeout and the optional external-display condition are design targets, not current functionality.

## Design

The single Rust daemon:

- directly holds logind's `handle-lid-switch:block` inhibitor FD across UPower restarts;
- subscribes to UPower `OnBattery` and `LidIsClosed` changes without polling;
- calls `org.freedesktop.login1.Manager.Suspend(false)` when required;
- rebuilds only its UPower subscription when UPower changes owner, and rebuilds the daemon-level inhibitor only after logind or the system bus changes.

There is no Python process, wrapper process, or `/etc/systemd/logind.conf` change.

## Build and validation

```bash
./test.sh
```

This creates the runtime binary at `bin/omarchy-power-policy`. The test suite starts an isolated private D-Bus with mock logind and UPower services, changes the UPower owner, and verifies that the same inhibitor FD remains open without any suspend request. A second isolated test starts, kills, and restarts the installed real `/usr/lib/upowerd` against that private bus while making the same inhibitor assertions. A safe live check acquires the real inhibitor but logs suspend decisions instead of executing them:

```bash
timeout 3s bin/omarchy-power-policy --dry-run
```

## Logs

```bash
journalctl --user -u omarchy-power-policy.service
```

Typical records:

```text
INFO inhibitor acquired what=handle-lid-switch mode=block
INFO state reason=upower:OnBattery on_battery=true lid_closed=true
WARN requesting suspend reason=upower:OnBattery
```

## Installation

After installing the repository as `~/.config/omarchy/plugins/fatlj.power-policy`, build it and install the user unit:

```bash
cd ~/.config/omarchy/plugins/fatlj.power-policy
./build.sh
install -Dm644 systemd/user/omarchy-power-policy.service \
  ~/.config/systemd/user/omarchy-power-policy.service
systemctl --user daemon-reload
systemctl --user enable --now omarchy-power-policy.service
omarchy plugin enable fatlj.power-policy
```

Verify the service owns exactly one `handle-lid-switch` inhibitor alongside Omarchy's separate sleep-delay inhibitor:

```bash
systemd-inhibit --list
systemctl --user status omarchy-power-policy.service
```

To remove it, disable the plugin and user unit, then delete the installed unit. No vendor files are modified.
