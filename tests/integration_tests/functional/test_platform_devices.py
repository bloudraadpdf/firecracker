# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Exercise optional platform devices through the jailed VMM and normal API."""

import json
import os
import platform
import shutil
import socket
import stat
import struct
import subprocess
import tempfile
from pathlib import Path

import pytest
from tenacity import retry, stop_after_delay, wait_fixed

import host_tools.cargo_build as host
from framework import utils
from framework.artifacts import GUEST_KERNEL_DEFAULT, pin_guest_kernel
from framework.defs import LOCAL_BUILD_PATH


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


@pytest.mark.skipif(platform.machine() != "x86_64", reason="AMD64 platform")
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


@pytest.mark.skipif(platform.machine() != "x86_64", reason="AMD64 platform")
def test_console_boot_and_restore(
    microvm_factory,
    platform_boot_image,
    tmp_path,
    windows_binaries,
    default_binary_dir,
    record_property,
):
    """Boot a real PVH guest with console/media, then verify a fresh VMM restore."""
    vm = microvm_factory.build(pci=True, **windows_binaries)
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
        "display": "vga",
        "optical_drives": [
            {"drive_id": "setup", "path_on_host": vm.create_jailed_resource(media)},
            {"drive_id": "exec", "path_on_host": None},
        ],
    }
    _configure(vm, platform_boot_image, {"console": config})
    assert vm.api.vm_config.get().json()["console"] == config
    frame = _frame(Path(vm.chroot()) / "console.sock")
    snapshot = _snapshot(vm)
    vm.kill()
    restored = microvm_factory.build(pci=True, **windows_binaries)
    restored.create_jailed_resource(rom)
    restored.create_jailed_resource(media)
    _load(restored, snapshot)
    assert _frame(Path(restored.chroot()) / "console.sock") == frame
    restored.kill()

    record_property("default_firecracker_bin", str(default_binary_dir / "firecracker"))
    rejected = microvm_factory.build(
        pci=True,
        fc_binary_path=default_binary_dir / "firecracker",
        jailer_binary_path=default_binary_dir / "jailer",
    )
    with pytest.raises(RuntimeError, match="windows build feature"):
        _load(rejected, snapshot)
    rejected.mark_killed()
    assert not (Path(rejected.chroot()) / "console.sock").exists()


@retry(stop=stop_after_delay(5), wait=wait_fixed(0.01), reraise=True)
def _ready(process, path, vm, *, mode=None):
    assert process.poll() is None
    assert path.is_socket()
    if mode is not None:
        assert stat.S_IMODE(path.stat().st_mode) == mode
    os.chown(path, -1, vm.jailer.gid)
    path.chmod(0o660)


@pytest.fixture(scope="module")
def variable_helper():
    """Build the separate Rust helper with its pinned dependencies."""
    target = f"{platform.machine()}-unknown-linux-gnu"
    host.cargo("build", f"--locked --target {target}", cwd="../tools/uefi-vars-helper")
    return LOCAL_BUILD_PATH / "cargo_target" / target / "debug/firecracker-uefi-vars"


@pytest.fixture
def platform_helpers(microvm_factory, variable_helper):
    """Own helper processes and stop VMMs before terminating their backends."""
    processes = []
    directories = []
    mounts = []

    def start(vm, source=None):
        directory = tempfile.TemporaryDirectory(prefix="pf-", dir=LOCAL_BUILD_PATH)
        directories.append(directory)
        root = Path(directory.name)
        state = root / "variables.json"
        if source is None:
            state.write_text(
                json.dumps(
                    {
                        "version": 2,
                        "identity": [81] * 16,
                        "generation": 0,
                        "state": {"flags": 0, "variables": [], "policies": []},
                    }
                ),
                encoding="utf-8",
            )
        else:
            shutil.copyfile(source, state)
        tpm_state = root / "tpm"
        tpm_state.mkdir(mode=0o700)
        tpm_socket = root / "tpm.sock"
        variables_socket = root / "vars.sock"
        commands = [
            [
                "swtpm",
                "socket",
                "--tpm2",
                "--tpmstate",
                f"dir={tpm_state}",
                "--ctrl",
                f"type=unixio,path={tpm_socket}",
                "--flags",
                "not-need-init,disable-auto-shutdown",
            ],
            [str(variable_helper), "serve", str(state), str(variables_socket)],
        ]
        os.chown(root, -1, vm.jailer.gid)
        root.chmod(0o710)
        for command, path in zip(commands, [tpm_socket, variables_socket]):
            process = subprocess.Popen(command)  # pylint: disable=consider-using-with
            processes.append(process)
            _ready(process, path, vm, mode=0o600 if path == variables_socket else None)
        mount = Path(vm.chroot()) / "helpers"
        mount.mkdir(parents=True)
        utils.check_output(["mount", "--bind", str(root), str(mount)], shell=False)
        mounts.append(mount)
        return (
            {"socket_path": "/helpers/tpm.sock"},
            {"socket_path": "/helpers/vars.sock"},
            state,
            processes[-1],
        )

    try:
        yield start
    finally:
        for vm in microvm_factory.vms:
            vm.kill()
        for process in processes:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)
        for mount in reversed(mounts):
            utils.check_output(["umount", str(mount)], shell=False)
        for directory in directories:
            directory.cleanup()


@pytest.mark.skipif(platform.machine() != "x86_64", reason="AMD64 platform")
def test_protected_services_restore_generation(
    microvm_factory,
    platform_boot_image,
    platform_helpers,
    windows_binaries,
    default_binary_dir,
    record_property,
):
    """Restore real swtpm/helper state and reject a stale destination generation."""
    vm = microvm_factory.build(pci=True, **windows_binaries)
    tpm, variables, store, _helper = platform_helpers(vm)
    _configure(vm, platform_boot_image, {"tpm": tpm, "uefi-vars": variables})
    snapshot = _snapshot(vm)
    vm.kill()

    record_property("default_firecracker_bin", str(default_binary_dir / "firecracker"))
    rejected = microvm_factory.build(
        pci=True,
        fc_binary_path=default_binary_dir / "firecracker",
        jailer_binary_path=default_binary_dir / "jailer",
    )
    with pytest.raises(RuntimeError, match="windows build feature"):
        _load(rejected, snapshot)
    rejected.mark_killed()

    generation = json.loads(store.read_text(encoding="utf-8"))["generation"]
    restored = microvm_factory.build(pci=True, **windows_binaries)
    tpm, variables, destination, _helper = platform_helpers(restored, store)
    restored.spawn(serial_out_path=None)
    state, memory = (restored.create_jailed_resource(path) for path in snapshot)
    request = {
        "snapshot_path": state,
        "mem_backend": {"backend_type": "File", "backend_path": memory},
        "resume_vm": True,
        "tpm": tpm,
        "uefi_vars": {**variables, "expected_generation": generation + 1},
    }
    with pytest.raises(RuntimeError, match="[Vv]ariable|[Ff]irmware"):
        restored.api.snapshot_load.put(**request)
    assert (
        json.loads(destination.read_text(encoding="utf-8"))["generation"] == generation
    )

    restored.kill()
    restored = microvm_factory.build(pci=True, **windows_binaries)
    tpm, variables, destination, helper = platform_helpers(restored, store)
    _load(
        restored,
        snapshot,
        tpm=tpm,
        uefi_vars={**variables, "expected_generation": generation},
    )
    assert (
        json.loads(destination.read_text(encoding="utf-8"))["generation"]
        == generation + 1
    )
    assert restored.api.vm_config.get().json()["tpm"] == tpm
    assert restored.api.vm_config.get().json()["uefi-vars"] == variables
    helper.terminate()
    helper.wait(timeout=5)
    restored.pause()
    with pytest.raises(RuntimeError, match="[Vv]ariable|[Ff]irmware|[Dd]evice"):
        restored.api.snapshot_create.put(
            snapshot_type="Full", snapshot_path="failed", mem_file_path="failed.mem"
        )
    restored.check_log_message("UEFI variable service failed")
    restored.mark_killed()


@pin_guest_kernel(GUEST_KERNEL_DEFAULT)
@pytest.mark.parametrize(
    "windows_source", [False, True], ids=["default-to-windows", "windows-to-default"]
)
def test_linux_snapshot_across_windows_capability(
    microvm_factory,
    guest_kernel,
    rootfs,
    pci_enabled,
    windows_binary_dir,
    default_binary_dir,
    windows_source,
    record_property,
):
    """Restore an ordinary Linux snapshot in the other capability build."""
    source_dir, restore_dir = (
        (windows_binary_dir, default_binary_dir)
        if windows_source
        else (default_binary_dir, windows_binary_dir)
    )
    record_property("snapshot_source_bin", str(source_dir / "firecracker"))
    record_property("snapshot_restore_bin", str(restore_dir / "firecracker"))
    vm = microvm_factory.build(
        guest_kernel,
        rootfs,
        pci=pci_enabled,
        fc_binary_path=source_dir / "firecracker",
        jailer_binary_path=source_dir / "jailer",
    )
    vm.spawn()
    vm.basic_config()
    vm.add_net_iface()
    vm.start()
    vm.ssh.check_output("true")
    snapshot = vm.snapshot_full()
    vm.kill()

    restored = microvm_factory.build(
        pci=pci_enabled,
        fc_binary_path=restore_dir / "firecracker",
        jailer_binary_path=restore_dir / "jailer",
    )
    restored.spawn()
    restored.restore_from_snapshot(snapshot, resume=True)
    assert restored.state == "Running"
    restored.ssh.check_output("true")
