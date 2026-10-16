# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Exercise optional platform devices through the jailed VMM and normal API."""

import platform
from pathlib import Path

import pytest

import host_tools.cargo_build as host
from framework.defs import LOCAL_BUILD_PATH

pytestmark = pytest.mark.skipif(platform.machine() != "x86_64", reason="AMD64 platform")


@pytest.fixture(scope="module")
def windows_binary_dir(pytestconfig):
    """Use the supplied binary pair or build the optional Windows capability."""
    binary_dir = pytestconfig.getoption("--binary-dir")
    return Path(binary_dir) if binary_dir else host.build_windows()


@pytest.fixture(scope="module")
def default_binary_dir():
    """Build the counterpart without the optional Windows capability."""
    build_path = LOCAL_BUILD_PATH / "platform-default"
    host.cargo(
        "build",
        f"--locked --target {host.DEFAULT_TARGET} --all --release",
        env={"CARGO_TARGET_DIR": build_path},
    )
    return build_path / host.DEFAULT_TARGET / "release"


@pytest.fixture
def windows_binaries(windows_binary_dir, record_property):
    """Keep VM ownership and teardown in the normal microVM factory."""
    record_property("windows_firecracker_bin", str(windows_binary_dir / "firecracker"))
    return {
        "fc_binary_path": windows_binary_dir / "firecracker",
        "jailer_binary_path": windows_binary_dir / "jailer",
    }


def test_hyperv_configuration(
    microvm_factory, windows_binaries, default_binary_dir, record_property
):
    """Enable Hyper-V only in the optional Windows build."""
    vm = microvm_factory.build(**windows_binaries)
    vm.spawn()
    vm.api.machine_config.put(mem_size_mib=128, vcpu_count=1)
    assert vm.api.machine_config.get().json()["hyperv"] is False
    vm.api.machine_config.patch(hyperv=True)
    vm.api.machine_config.patch(mem_size_mib=256)
    assert vm.api.machine_config.get().json()["hyperv"] is True
    vm.api.machine_config.put(mem_size_mib=128, vcpu_count=1)
    assert vm.api.machine_config.get().json()["hyperv"] is False

    record_property("default_firecracker_bin", str(default_binary_dir / "firecracker"))
    default = microvm_factory.build(
        fc_binary_path=default_binary_dir / "firecracker",
        jailer_binary_path=default_binary_dir / "jailer",
    )
    default.spawn()
    default.api.machine_config.put(mem_size_mib=128, vcpu_count=1)
    before = default.api.machine_config.get().json()
    for update in (default.api.machine_config.put, default.api.machine_config.patch):
        with pytest.raises(RuntimeError, match="windows build feature"):
            update(mem_size_mib=256, vcpu_count=1, hyperv=True)
        assert default.api.machine_config.get().json() == before


