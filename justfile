# Uguisu task runner. `just` is optional: every check is also
# `python3 scripts/check.py <name>`, which is what CI runs.

set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

# Format, lint, test, web and docs, with one line per check
check *ARGS:
    python3 scripts/check.py {{ARGS}}

# Everything, including deny, bench, smoke and the migration guard
check-all:
    python3 scripts/check.py all

fmt-check:
    python3 scripts/check.py fmt

clippy:
    python3 scripts/check.py clippy

test:
    python3 scripts/check.py test

deny:
    python3 scripts/check.py deny

web-check:
    python3 scripts/check.py web

web-build:
    python3 scripts/check.py web-build

docs-links:
    python3 scripts/check.py docs

smoke:
    python3 scripts/check.py smoke

# Rewrite formatting in place, rather than checking it
fmt:
    cargo fmt --all

build:
    cargo build --workspace --all-targets

# The real benchmark run; `check.py bench` only proves they compile
bench:
    cargo bench --workspace

# Record a live provider fixture, e.g. `just record provider apple --case search_darknet --query "Darknet Diaries"`
record *ARGS:
    cargo run -p record-fixtures -- {{ARGS}}

# Run the uguisu binary, e.g. `just run -- serve`
run *ARGS:
    cargo run -p uguisu-cli -- {{ARGS}}

web-install:
    cd web && pnpm install

web-dev:
    cd web && pnpm dev
