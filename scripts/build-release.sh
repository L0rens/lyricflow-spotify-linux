#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
release_target="${LYRICFLOW_RELEASE_TARGET:-x86_64-unknown-linux-gnu}"
release_version="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "lyricflow"))')"
release_name="lyricflow-v${release_version}-${release_target}"
release_target_dir="${CARGO_TARGET_DIR:-target}"
release_stage="$(mktemp -d)"
trap 'rm -rf -- "$release_stage"' EXIT

cargo build --release --locked --target "$release_target"
mkdir -p "$release_stage/$release_name" dist
install -m755 "$release_target_dir/$release_target/release/lyricflow" "$release_stage/$release_name/lyricflow"
install -m644 LICENSE README.md CHANGELOG.md assets/demo.lrc "$release_stage/$release_name/"
mkdir -p "$release_stage/$release_name/docs/images"
install -m644 docs/images/lyricflow.gif "$release_stage/$release_name/docs/images/lyricflow.gif"
tar --sort=name --owner=0 --group=0 --numeric-owner -czf "dist/$release_name.tar.gz" -C "$release_stage" "$release_name"
(
    cd dist
    sha256sum "$release_name.tar.gz" > SHA256SUMS
)
printf 'Built dist/%s.tar.gz and dist/SHA256SUMS\n' "$release_name"
