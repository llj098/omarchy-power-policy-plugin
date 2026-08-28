#!/bin/bash
set -euo pipefail

repo=$(cd -- "$(dirname -- "$0")" && pwd)
cd "$repo"

python -m unittest discover -s tests -v
python -m py_compile bin/omarchy-power-policy
python -m json.tool manifest.json >/dev/null

if command -v qmlformat6 >/dev/null 2>&1; then
  qmlformat6 -n Service.qml >/dev/null
elif [[ -x /usr/lib/qt6/bin/qmlformat ]]; then
  /usr/lib/qt6/bin/qmlformat -n Service.qml >/dev/null
fi

systemd-analyze --user --man=no verify systemd/user/omarchy-power-policy.service

echo "power_policy_tests=ok"
