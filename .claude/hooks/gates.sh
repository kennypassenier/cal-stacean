#!/usr/bin/env bash
# Almanac quality gates (standing rules 6/7): format, lint with
# warnings as errors, full test suite, and the AR13 module boundary.
# Called by .githooks/pre-commit and .claude/hooks/check-commit.sh;
# non-zero exit blocks the commit.
set -euo pipefail

# ── Standing rule 7: a gate that does not predict the build is not a gate ──
# The checks below rewrite files. cargo updates Cargo.lock, formatters
# rewrite sources — and anything rewritten AFTER `git add` is green here
# and absent from the commit. kyu's 1.0.0 commit carried a lock file
# still naming version 0.0.0; the container build refused it one step
# before a release tag, and nothing local had objected. So: fingerprint
# the tree now, compare once the checks are done, and refuse rather than
# report a green run over a tree that moved underneath it.
gate_tree_fingerprint() {
  { git status --porcelain; git diff; } | sha256sum | cut -d' ' -f1
}
gate_tree_before=$(gate_tree_fingerprint)
cd "$(git rev-parse --show-toplevel)"

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all

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

# Standing rule 7, second clause: see gate_tree_fingerprint above.
if [ "$(gate_tree_fingerprint)" != "$gate_tree_before" ]; then
  {
    echo "gates: the checks rewrote the working tree while they ran."
    echo "A file changed after it was staged, so what this commit carries is"
    echo "NOT what was just tested. Most often this is cargo refreshing"
    echo "Cargo.lock; the changed paths are listed below."
    echo
    git status --porcelain
    echo
    echo "What now: run 'git add -A' and commit again."
  } >&2
  exit 1
fi
