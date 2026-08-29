#!/bin/bash
set -euo pipefail

repo=$(cd -- "$(dirname -- "$0")" && pwd)
cd "$repo"

cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./build.sh
python -m json.tool manifest.json >/dev/null

if command -v qmlformat6 >/dev/null 2>&1; then
  qmlformat6 -n Service.qml >/dev/null
elif [[ -x /usr/lib/qt6/bin/qmlformat ]]; then
  /usr/lib/qt6/bin/qmlformat -n Service.qml >/dev/null
fi

unit=$(mktemp --suffix=.service)
trap 'rm -f -- "$unit"' EXIT
sed "s|%h/.config/omarchy/plugins/fatlj.power-policy|$repo|g" \
  systemd/user/omarchy-power-policy.service >"$unit"
systemd-analyze --user --man=no verify "$unit"

echo "power_policy_tests=ok implementation=rust"
