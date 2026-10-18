# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Shared Windows controller fixtures for build checks."""

import re
from pathlib import Path

import pytest


@pytest.fixture
def windows_cleanup():
    """Read actual shell cleanup functions without starting their controllers."""
    windows = Path(__file__).resolve().parents[3] / "tools/windows"
    controller = (windows / "legacy-vsock/check.sh").read_text()
    secure = (windows / "egress/secure-state.sh").read_text()
    tap = (windows / "egress/tap-check.sh").read_text()
    functions = {}
    for source, prefix, names in [
        (
            controller,
            "",
            ["cleanup_failed", "cleanup", "stop_vm", "remove_sockets", "wait_for_exit"],
        ),
        (secure, "", ["secure_discard", "secure_stop", "secure_wait_stopped"]),
        (tap, "tap_", ["cleanup_failed", "cleanup", "stop"]),
    ]:
        for name in names:
            match = re.search(r"^" + name + r"\(\) \{\n.*?^\}", source, re.M | re.S)
            if match is None and name == "cleanup_failed":
                continue
            assert match is not None
            functions[prefix + name] = match.group()
    return functions
