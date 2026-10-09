#!/usr/bin/env python3
"""Refuse a Dockerfile that breaks a rule ADR 0061 sets, without Docker.

Read as text, so it runs anywhere python does, before anything is built:

- every base image is pinned by digest, and the Rust one is the toolchain
  `rust-toolchain.toml` pins;
- the image runs as uid 65532, never as root;
- its health check is `uguisu health`, and SIGTERM stops it;
- the bind, data, media and web paths are set, and the exposure override is
  not (ADR 0037);
- `.dockerignore` keeps build output, dependencies, `.git`, env files and a
  local database or archive out of the context, so neither a stale binary
  nor a secret is sent to it.

`scripts/docker_smoke.py` builds and runs the image; this only reads it.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

ENV = {
    "UGUISU_BIND": "0.0.0.0:8484",
    "UGUISU_DATA_DIR": "/data",
    "UGUISU_MEDIA_DIR": "/media/podcasts",
    "UGUISU_WEB_DIR": "/usr/share/uguisu/web",
}
IGNORED = [
    ".git",
    "target",
    "**/node_modules",
    "web/dist",
    "desktop",
    "**/*.env",
    "**/.env*",
    "data",
    "podcasts",
    "**/*.db",
]


def instructions(text: str) -> list[tuple[str, str]]:
    """The Dockerfile's instructions, continuation lines joined, comments dropped."""
    joined = re.sub(r"\\\n", " ", text)
    out = []
    for line in joined.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            word, _, rest = line.partition(" ")
            out.append((word.upper(), rest.strip()))
    return out


def main() -> int:
    dockerfile = (ROOT / "Dockerfile").read_text(encoding="utf-8")
    steps = instructions(dockerfile)
    problems: list[str] = []

    bases = [rest.split()[0] for word, rest in steps if word == "FROM"]
    for base in bases:
        if "@sha256:" not in base:
            problems.append(f"FROM {base} is not pinned by digest")
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))[
        "toolchain"
    ]["channel"]
    if not any(base.startswith(f"rust:{channel}-") for base in bases):
        problems.append(f"no rust:{channel}-… stage, the toolchain rust-toolchain.toml pins")

    final = steps[max(i for i, (word, _) in enumerate(steps) if word == "FROM") :]
    users = [rest for word, rest in final if word == "USER"]
    if users[-1:] != ["65532:65532"]:
        problems.append(f"the final stage runs as {users[-1:] or 'root'}, not 65532:65532")
    health = [rest for word, rest in final if word == "HEALTHCHECK"]
    if not health or '"uguisu", "health"' not in health[-1].replace("/usr/local/bin/uguisu", "uguisu"):
        problems.append("the health check is not `uguisu health`")
    if ("STOPSIGNAL", "SIGTERM") not in final:
        problems.append("STOPSIGNAL is not SIGTERM")

    env_text = " ".join(rest for word, rest in final if word == "ENV")
    for key, value in ENV.items():
        if f"{key}={value}" not in env_text:
            problems.append(f"ENV does not set {key}={value}")
    # The variable and the flag alike: `--allow-insecure-exposure` is the same override.
    if re.search(r"insecure[-_]exposure", dockerfile, re.IGNORECASE):
        problems.append("the image sets the exposure override (ADR 0037)")

    ignore = (ROOT / ".dockerignore").read_text(encoding="utf-8").split()
    for pattern in IGNORED:
        if pattern not in ignore:
            problems.append(f".dockerignore does not exclude {pattern}")

    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
