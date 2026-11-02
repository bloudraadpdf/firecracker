# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Reject active backing images before Windows tests or offline verification."""

import json
import os

import pytest

from framework import utils
from framework.defs import FC_WORKSPACE_DIR

GUARD = FC_WORKSPACE_DIR / "tools/windows/egress/assert-disk-closed.sh"


@pytest.fixture
def disk(tmp_path):
    """Create an owned sparse image without a filesystem or mount."""
    image = tmp_path / "disk image.raw"
    with image.open("xb") as backing:
        backing.truncate(1024 * 1024)
    return image


def _guard(*args):
    return utils.run_cmd(["bash", str(GUARD), *map(str, args)], shell=False)


@pytest.mark.parametrize("offset", [0, 512])
def test_windows_disk_guard_rejects_loop_associations(disk, tmp_path, offset):
    """Detect inode aliases and offset loops after losetup closes its file."""
    hardlink = tmp_path / "hard link.raw"
    hardlink.hardlink_to(disk)
    symlink = tmp_path / "symlink.raw"
    symlink.symlink_to(disk)
    config = tmp_path / "config.json"
    config.write_text(
        json.dumps(
            {"drives": [{"path_on_host": str(hardlink), "is_read_only": False}]}
        ),
        encoding="utf-8",
    )
    assert _guard(disk).returncode == 0
    _, stdout, _ = utils.check_output(
        [
            "losetup",
            "--find",
            "--show",
            "--read-only",
            "--nooverlap",
            "--offset",
            str(offset),
            str(disk),
        ],
        shell=False,
    )
    loop = stdout.strip()
    try:
        for path in [disk, hardlink, symlink]:
            result = _guard(path)
            assert result.returncode != 0
            assert "attached to a loop device" in result.stderr
        assert _guard("--config", config).returncode != 0
    finally:
        utils.check_output(["losetup", "--detach", loop], shell=False)
    assert _guard(disk).returncode == 0


def test_windows_disk_guard_rejects_open_file(disk):
    """Retain the descriptor check for an ordinary process using the image."""
    with disk.open("rb"):
        result = _guard(disk)
        assert result.returncode != 0
        assert "still open" in result.stderr
    assert _guard(disk).returncode == 0


def test_windows_disk_guard_rejects_loop_inspection_failure(
    disk, tmp_path, monkeypatch
):
    """An inspection error must not become a closed-image result."""
    binary = tmp_path / "losetup"
    binary.write_text("#!/bin/sh\nexit 7\n", encoding="utf-8")
    binary.chmod(0o700)
    monkeypatch.setenv("PATH", f"{tmp_path}:{os.environ['PATH']}")
    result = _guard(disk)
    assert result.returncode != 0
    assert "Cannot inspect loop devices" in result.stderr
