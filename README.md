# Omarchy Power Policy

A focused Omarchy service plugin for one policy:

| State transition | Action |
|---|---|
| Lid closes while on AC | Keep running |
| Lid closes while on battery | Suspend |
| AC is unplugged while the lid is closed | Suspend |
| AC is unplugged while the lid is open | No action |

External-monitor state does not override this policy. Omarchy continues to own clamshell display handling, idle/screensaver behavior, and its existing `sleep:delay` lock-before-suspend path.

## Design

The single Rust daemon:

- directly holds logind's `handle-lid-switch:block` inhibitor FD;
- subscribes to UPower `OnBattery` and `LidIsClosed` changes without polling;
- calls `org.freedesktop.login1.Manager.Suspend(false)` when required;
- rebuilds its D-Bus epoch and inhibitor after logind or UPower owner changes.

There is no Python process, wrapper process, or `/etc/systemd/logind.conf` change.

## Build and validation

```bash
./test.sh
```

This creates the runtime binary at `bin/omarchy-power-policy`. A safe live check acquires the real inhibitor but logs suspend decisions instead of executing them:

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
