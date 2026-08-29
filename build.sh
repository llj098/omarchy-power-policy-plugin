#!/bin/bash
set -euo pipefail

repo=$(cd -- "$(dirname -- "$0")" && pwd)
cd "$repo"

cargo build --release --locked
install -Dm755 target/release/omarchy-power-policy bin/omarchy-power-policy
