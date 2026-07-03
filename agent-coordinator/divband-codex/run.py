#!/usr/bin/env python3
"""Single entry point for applying the Divband Codex overlay."""

from __future__ import annotations

import sys
from pathlib import Path

from toolchain.orchestrator import main


if __name__ == "__main__":
    sys.exit(main(Path(__file__).resolve().parent))

