# Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
# SPDX-License-Identifier: Apache-2.0
"""A test that ensures that all unit tests pass at integration time."""

import os
import platform

import pytest

import host_tools.cargo_build as host  # pylint:disable=import-error
from framework import utils_repo

MACHINE = platform.machine()
TARGET = "{}-unknown-linux-musl".format(MACHINE)


@pytest.mark.timeout(600)
@pytest.mark.parametrize(
    "features", ["", "--features windows"], ids=["default", "windows"]
)
def test_unittests(test_fc_session_root_path, features):
    """
    Run unit and doc tests for all supported targets.
    """
    extra_args = f"--target {TARGET} {features}"
    host.cargo_test(test_fc_session_root_path, extra_args=extra_args)
    host.cargo_test(test_fc_session_root_path, extra_args=extra_args + " --examples")


@pytest.mark.parametrize(
    "features", ["", "--features windows"], ids=["default", "windows"]
)
def test_benchmarks_compile(features):
    """Checks that all benchmarks compile"""

    host.cargo("bench", f"--all --no-run --target {TARGET} {features}")


@pytest.mark.parametrize(
    "directory", utils_repo.rust_tool_directories(), ids=lambda p: p.name
)
@pytest.mark.timeout(600)
def test_tool_unittests(directory):
    """Run standalone tool unit tests and compile their examples."""
    host.cargo(
        "test",
        f"--locked --all-targets --target {MACHINE}-unknown-linux-gnu",
        cwd=directory,
        timeout=540,
    )
