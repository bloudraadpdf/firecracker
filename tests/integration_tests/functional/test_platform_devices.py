# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Exercise optional platform devices through the jailed VMM and normal API."""

import json
import platform
import socket
import struct
from pathlib import Path

import pytest
from tenacity import retry, stop_after_delay, wait_fixed

from framework import utils

pytestmark = pytest.mark.skipif(platform.machine() != "x86_64", reason="AMD64 platform")


@pytest.fixture(scope="module")
def platform_boot_image(tmp_path_factory):
    """Compile a distributable PVH image that writes VGA memory, then halts."""
    output = tmp_path_factory.mktemp("platform") / "boot.elf"
    utils.check_output(
        [
            "rustc",
            "--edition=2024",
            "-Cpanic=abort",
            "-Crelocation-model=static",
            "-Clinker=gcc",
            "-Clink-arg=-nostdlib",
            "-Clink-arg=-static",
            f"-Clink-arg=-Wl,-T,{Path('data/platform/boot.ld').resolve()}",
            "data/platform/boot.rs",
            "-o",
            str(output),
        ],
        shell=False,
    )
    return output


def _configure(vm, boot_image, config):
    kernel = vm.create_jailed_resource(boot_image)
    config = {
        "boot-source": {"kernel_image_path": kernel},
        "machine-config": {"vcpu_count": 1, "mem_size_mib": 64},
        "drives": [],
        **config,
    }
    path = Path(vm.chroot()) / "platform.json"
    path.write_text(json.dumps(config), encoding="utf-8")
    vm.jailer.extra_args["config-file"] = path.name
    vm.spawn(serial_out_path=None)
    assert vm.state == "Running"
    assert "Kernel loaded using PVH boot protocol" in vm.log_data


@retry(stop=stop_after_delay(5), wait=wait_fixed(0.01), reraise=True)
def _frame(path):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(path))
        client.sendall(b"FCC1" + bytes(8))
        with client.makefile("rb") as response:
            header = response.read(16)
            magic, width, height, size = struct.unpack("<4sIII", header)
            assert magic == b"FCF1"
            assert size == width * height * 4
            pixels = response.read(size)
            assert len(pixels) == size
            assert (width, height) == (2, 2)
            assert pixels == struct.pack("<4I", 0x112233, 0xAABBCC, 0, 0)
            return header + pixels


def _snapshot(vm):
    vm.pause()
    vm.api.snapshot_create.put(
        snapshot_type="Full", snapshot_path="state", mem_file_path="memory"
    )
    return Path(vm.chroot()) / "state", Path(vm.chroot()) / "memory"


def _load(vm, snapshot, **services):
    state, memory = (vm.create_jailed_resource(path) for path in snapshot)
    vm.spawn(serial_out_path=None)
    vm.api.snapshot_load.put(
        snapshot_path=state,
        mem_backend={"backend_type": "File", "backend_path": memory},
        resume_vm=True,
        **services,
    )
    assert vm.state == "Running"


def test_console_boot_and_restore(microvm_factory, platform_boot_image, tmp_path):
    """Boot a real PVH guest with console/media, then verify a fresh VMM restore."""
    vm = microvm_factory.build(pci=True)
    rom = tmp_path / "vga.rom"
    contents = bytearray(512)
    contents[:3] = b"\x55\xaa\x01"
    contents[-1] = -sum(contents) % 256
    rom.write_bytes(contents)
    media = tmp_path / "media.iso"
    media.write_bytes(bytes(2048))
    config = {
        "socket_path": "console.sock",
        "vga_rom_path": vm.create_jailed_resource(rom),
        "optical_image_path": vm.create_jailed_resource(media),
    }
    _configure(vm, platform_boot_image, {"console": config})
    assert vm.api.vm_config.get().json()["console"] == config
    frame = _frame(Path(vm.chroot()) / "console.sock")
    snapshot = _snapshot(vm)
    vm.kill()
    restored = microvm_factory.build(pci=True)
    restored.create_jailed_resource(rom)
    restored.create_jailed_resource(media)
    _load(restored, snapshot)
    assert _frame(Path(restored.chroot()) / "console.sock") == frame
    restored.kill()
