#!/usr/bin/env bash
# Almanac's own gates (chassis 1.6.0, M1): run by the kit's gates.sh and CI
# after fmt, clippy and the tests. This file is project-owned; `chassis
# sync` never touches it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# M8: one version. A tag that disagrees with Cargo.toml would make the
# self-updater either never update or update on every poll.
./scripts/check-version.sh >/dev/null

# AR13: the core module must stay free of ambient I/O. The compiler
# cannot enforce a module boundary inside a single crate, so this gate
# does — a hit below means I/O belongs behind a shell-injected trait.
if [ -d src/core ]; then
  if grep -rnE '^[[:space:]]*use[[:space:]]+(reqwest|axum|hyper|tokio::(fs|net|io)|std::(fs|net))' src/core/; then
    echo "GATE FAILED — src/core imports an I/O crate (AR13)." >&2
    echo "Move the I/O behind a trait implemented in the shell module." >&2
    exit 1
  fi
fi
