# Omarchy Power Policy

Focused Omarchy service plugin for one policy:

| State transition | Action |
|---|---|
| Lid closes while on AC | Keep running |
| Lid closes while on battery | Suspend |
| AC is unplugged while the lid is closed | Suspend |
| AC is unplugged while the lid is open | No action |

External-monitor state does not override this policy. Omarchy continues to own clamshell display handling, idle/screensaver behavior, and the existing `sleep:delay` lock-before-suspend path.

## Design

`bin/omarchy-power-policy` subscribes to UPower's `OnBattery` and `LidIsClosed` D-Bus properties. The user unit wraps it in a `handle-lid-switch:block` logind inhibitor so logind does not make a competing lid decision. When the policy requires sleep, the daemon calls `org.freedesktop.login1.Manager.Suspend(false)`.

There is no polling and no `/etc/systemd/logind.conf` change.

## Validation

From the repository:

```bash
./test.sh
```

A safe live D-Bus read can be checked without acquiring an inhibitor or suspending:

```bash
timeout 3s bin/omarchy-power-policy --dry-run
```

## Logs

The daemon logs only state changes and actions. Under the user service:

```bash
journalctl --user -u omarchy-power-policy.service
```

Typical records are:

```text
INFO state reason=upower:OnBattery on_battery=True lid_closed=True
WARNING requesting suspend reason=upower:OnBattery
```

## Installation

This repository is not deployed automatically. After installing it as `~/.config/omarchy/plugins/fatlj.power-policy`, install and enable the user unit:

```bash
install -Dm644 \
  ~/.config/omarchy/plugins/fatlj.power-policy/systemd/user/omarchy-power-policy.service \
  ~/.config/systemd/user/omarchy-power-policy.service
systemctl --user daemon-reload
systemctl --user enable --now omarchy-power-policy.service
omarchy plugin enable fatlj.power-policy
```

Verify that exactly one policy inhibitor exists alongside Omarchy's separate sleep-delay inhibitor:

```bash
systemd-inhibit --list
systemctl --user status omarchy-power-policy.service
```

To remove it, disable the plugin and unit, then delete the installed unit file. No vendor files are modified.
