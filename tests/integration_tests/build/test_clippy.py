# Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
# SPDX-License-Identifier: Apache-2.0
"""Tests ensuring codebase style compliance for Rust and Python."""

import platform

import pytest

from framework import utils_repo
from host_tools.cargo_build import cargo

SUCCESS_CODE = 0
MACHINE = platform.machine()
TARGETS = [
    "{}-unknown-linux-gnu".format(MACHINE),
    "{}-unknown-linux-musl".format(MACHINE),
]


@pytest.mark.parametrize("target", TARGETS)
def test_rust_clippy(target):
    """
    Test that clippy does not generate any errors/warnings.
    """
    cargo(
        "clippy", f"--target {target} --all --all-targets --all-features", "-D warnings"
    )


@pytest.mark.parametrize(
    "directory", utils_repo.rust_tool_directories(), ids=lambda p: p.name
)
def test_tool_clippy(directory):
    """Check standalone tools with their native C-library dependencies."""
    cargo(
        "clippy",
        f"--locked --target {MACHINE}-unknown-linux-gnu --all-targets --all-features",
        "-D warnings",
        cwd=directory,
    )
