import QtQuick
import Quickshell.Io

Item {
  id: root

  // Injected by the Omarchy plugin loader.
  property var shell: null

  Process {
    id: serviceStarter
    command: ["systemctl", "--user", "start", "omarchy-power-policy.service"]
  }

  Component.onCompleted: serviceStarter.running = true
}
