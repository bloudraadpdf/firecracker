# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check exact prearmed-generation evidence without a Windows guest."""

import contextlib
import hashlib
import json
import os
import re
import runpy
import socket
import subprocess
import sys
from pathlib import Path

import pytest

VERIFIER = Path(__file__).resolve().parents[3] / "tools/windows/generation/verify.py"
VERIFY = runpy.run_path(str(VERIFIER))
IDS = [digit * 32 for digit in "123"]


@pytest.fixture
def evidence(tmp_path):
    """Create cold/full/diff records with two existing pending child requests."""
    events = tmp_path / "events"
    events.mkdir()
    reports = []
    for iteration in [1, 2, 3]:
        if iteration > 1:
            reports.append(
                f"VMGENID_EVENT_COMPLETE iteration={iteration} "
                f"armed_iteration={iteration - 1} pid={1000 + iteration - 1} "
                f"old={IDS[iteration - 2]} new={IDS[iteration - 1]} exit=0"
            )
        if iteration < 3:
            reports.append(
                f"VMGENID_EVENT_ARMED iteration={iteration} "
                f"pid={1000 + iteration} old={IDS[iteration - 1]} "
                "alive=true timeout_ms=300000"
            )
            (events / f"iteration-{iteration}.stdout").write_bytes(
                f"ARMED VMGENID={IDS[iteration - 1]}\r\n"
                f"VMGENID={IDS[iteration]}\r\n".encode()
            )
            (events / f"iteration-{iteration}.stderr").write_bytes(b"")
        reports.append(
            f"PASS WIN7_NET iteration={iteration} bytes=2097152 "
            "DNS HTTPS TLS1.2 TCP_EQUAL STORAGE_SHA256="
            f"{'a' * 64} VMGENID={IDS[iteration - 1]} 10/17/2026 12:00:00"
        )
    _write_reports(tmp_path, reports)
    (tmp_path / "stages.log").write_text(
        "now phase=cold watermark=0\n"
        "now phase=cold completed=1\nnow phase=cold completed=1\n"
        "now phase=full watermark=2\n"
        "now phase=full completed=2\nnow phase=full completed=2\n"
        "now phase=diff watermark=5\nnow phase=diff completed=3\n",
        encoding="utf-8",
    )
    return tmp_path


def _write_reports(root, reports):
    """Write the same guest messages and host receive records."""
    (root / "events/reports.log").write_text(
        "\n".join(reports) + "\n", encoding="utf-8"
    )
    (root / "echo.log").write_text(
        "\n".join(
            f"report: {report} host_unix_ms={1000 + index}"
            for index, report in enumerate(reports)
        )
        + "\n",
        encoding="utf-8",
    )


@pytest.mark.parametrize("tag", ["WIN7", "SERVER"])
def test_windows_generation_verifies_exact_evidence(evidence, tag):
    """Use supplied tags and hash the same raw bytes that were parsed."""
    for name in ["events/reports.log", "echo.log"]:
        path = evidence / name
        path.write_text(
            path.read_text(encoding="utf-8").replace("WIN7_NET", f"{tag}_NET"),
            encoding="utf-8",
        )
    result = VERIFY["verify"](evidence, tag)
    assert result["generation_ids"] == IDS
    assert result["armed"][1]["pid"] == result["complete"][2]["pid"]
    assert result["armed"][2]["pid"] == result["complete"][3]["pid"]
    assert len(result["host_report_records"]) == 7
    for name, item in result["guest_evidence"].items():
        data = (evidence / "events" / name).read_bytes()
        assert item == {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


@pytest.mark.parametrize(
    "change,diagnostic",
    [
        ("duplicate", "seven guest reports"),
        ("missing", "seven guest reports"),
        ("order", "ARMED request must precede PASS"),
        ("pid", "same-PID successful completion"),
        ("unchanged", "nonzero and distinct"),
        ("zero", "nonzero and distinct"),
        ("not_alive", "live ARMED request"),
        ("timeout", "live ARMED request"),
    ],
)
def test_windows_generation_rejects_invalid_reports(evidence, change, diagnostic):
    """Reject duplicates, reordered boundaries and a replacement child process."""
    reports = (evidence / "events/reports.log").read_text(encoding="utf-8").splitlines()
    if change == "duplicate":
        reports.append(reports[-1])
    elif change == "missing":
        reports.pop()
    elif change == "order":
        reports[0], reports[1] = reports[1], reports[0]
    elif change == "pid":
        reports[2] = reports[2].replace("pid=1001", "pid=9999")
    elif change == "unchanged":
        reports = [line.replace(IDS[1], IDS[0]) for line in reports]
    elif change == "zero":
        reports = [line.replace(IDS[0], "0" * 32) for line in reports]
    elif change == "not_alive":
        reports[0] = reports[0].replace("alive=true", "alive=false")
    elif change == "timeout":
        reports[0] = reports[0].replace("timeout_ms=300000", "timeout_ms=1")
    _write_reports(evidence, reports)
    with pytest.raises(ValueError, match=diagnostic):
        VERIFY["verify"](evidence, "WIN7")


@pytest.mark.parametrize(
    "name,old,new,diagnostic",
    [
        ("echo.log", " host_unix_ms=1000", "", "receipt timestamp"),
        ("echo.log", " host_unix_ms=1000", " host_unix_ms=bad", "receipt timestamp"),
        (
            "echo.log",
            " host_unix_ms=1000",
            " host_unix_ms=1 host_unix_ms=2",
            "receipt timestamp",
        ),
        ("events/reports.log", "alive=true", "alive=false", "reports differ"),
        ("events/iteration-1.stdout", IDS[1], IDS[2], "child output"),
        ("events/iteration-2.stderr", "", "unexpected", "stderr is not empty"),
        (
            "stages.log",
            "phase=full watermark=2",
            "phase=full watermark=0",
            "Cold ARMED/PASS",
        ),
        (
            "stages.log",
            "phase=diff completed=3",
            "phase=diff completed=2",
            "diff completion count",
        ),
    ],
)
def test_windows_generation_rejects_inconsistent_files(
    evidence, name, old, new, diagnostic
):
    """Keep received timestamps, child output and controller boundaries strict."""
    path = evidence / name
    path.write_text(
        path.read_text(encoding="utf-8").replace(old, new), encoding="utf-8"
    )
    with pytest.raises(ValueError, match=diagnostic):
        VERIFY["verify"](evidence, "WIN7")


def test_windows_generation_rejects_partial_report(evidence):
    """An unfinished last record cannot qualify a closed guest run."""
    path = evidence / "events/reports.log"
    path.write_bytes(path.read_bytes()[:-1])
    with pytest.raises(ValueError, match="Incomplete guest reports.log"):
        VERIFY["verify"](evidence, "WIN7")


@pytest.mark.parametrize("change", ["extra", "symlink", "oversize"])
def test_windows_generation_rejects_unbounded_collection(evidence, tmp_path, change):
    """Copy only the exact bounded regular evidence files from the mount."""
    source = evidence / "events"
    if change == "extra":
        (source / "extra").touch()
    elif change == "symlink":
        path = source / "iteration-1.stderr"
        path.unlink()
        path.symlink_to(source / "iteration-2.stderr")
    else:
        (source / "iteration-1.stdout").write_bytes(b"x" * 4097)
    target = tmp_path / "collected"
    target.mkdir()
    with pytest.raises(ValueError):
        VERIFY["collect"](source, target)
    assert not (target / "events").exists()


def test_windows_generation_collection_and_optimized_cli(evidence, tmp_path):
    """Preserve raw bytes and enforce checks under Python optimisation too."""
    target = tmp_path / "collected"
    target.mkdir()
    for name in ["echo.log", "stages.log"]:
        (target / name).write_bytes((evidence / name).read_bytes())
    command = [sys.executable, "-O", str(VERIFIER), str(target), "--tag", "WIN7"]
    result = subprocess.run(
        command + ["--collect-from", str(evidence / "events")],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert json.loads(result.stdout)["generation_ids"] == IDS
    for name in VERIFY["EVENT_FILES"]:
        assert (target / "events" / name).read_bytes() == (
            evidence / "events" / name
        ).read_bytes()
    with pytest.raises(FileExistsError):
        VERIFY["collect"](evidence / "events", target)
    (target / "events/iteration-1.stderr").write_bytes(b"failure")
    result = subprocess.run(
        command, check=False, capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 1
    assert "stderr is not empty for iteration 1" in result.stderr
    assert "Traceback" not in result.stderr


@pytest.fixture(params=["tap", "connect"])
def xp_evidence(tmp_path, request):
    """Keep inline counter reads separate from the existing event protocol."""
    mode = request.param
    reader = tmp_path / "reader.exe"
    reader.write_bytes(b"pinned XP reader")
    identity = {
        "bytes": reader.stat().st_size,
        "sha256": hashlib.sha256(reader.read_bytes()).hexdigest(),
    }
    (tmp_path / "generation-reader-before.json").write_text(json.dumps(identity))
    transcripts = []
    for iteration, value in enumerate(IDS, 1):
        records = (
            f"VMGENID_READ iteration={iteration} read=1 value={value}\n"
            f"VMGENID_READ iteration={iteration} read=2 value={value}\n"
            f"VMGENID_STAGE iteration={iteration} value={value}\nVMGENID={value}\n"
        )
        marker = "XP_CHECK" if mode == "tap" else "XPLEGACY_PASS"
        transcripts.append(records + f"{marker} iteration={iteration}\n")
        (tmp_path / f"{['cold', 'full', 'diff'][iteration - 1]}.log").write_text(
            transcripts[-1]
        )
        if iteration == 3:
            (tmp_path / "final-guest.txt").write_text(records)
    (tmp_path / "guest-transcript.txt").write_text("".join(transcripts))
    (tmp_path / "echo.log").write_text(
        "".join(f"received {i}\necho: peer=guest bytes=2097152\n" for i in range(3))
    )
    (tmp_path / "proxy.log").write_text("READY\n" + "egress CONNECT rejected\n" * 3)
    marks = [0, 2, 4] if mode == "tap" else [1, 2, 3]
    stages = []
    for index, phase in enumerate(["cold", "full", "diff"]):
        stages.append(f"now phase={phase} watermark={marks[index]}\n")
        stages += [f"now phase={phase} completed={index + 1}\n"] * (
            2 if index < 2 else 1
        )
    (tmp_path / "stages.log").write_text("".join(stages))
    return tmp_path, mode, reader, identity["sha256"]


def test_xp_polling_verifies_actual_reader_and_stage_pairs(xp_evidence):
    """Require six real read records, three IDs and the prelaunch reader pin."""
    root, mode, reader, digest = xp_evidence
    result = VERIFY["verify_xp"](root, mode, reader, digest)
    assert result["generation_ids"] == IDS
    assert result["reads"] == 6
    assert result["reader"]["sha256"] == digest
    assert result["notifications_qualified"] is False


@pytest.mark.parametrize("xp_evidence", ["tap"], indirect=True)
@pytest.mark.parametrize(
    "change",
    [None, "two-spaces", "tab", "text", "leading", "duplicate", "before-polls"],
)
def test_xp_tap_accepts_exact_cmd_serial_marker(xp_evidence, change):
    """Match CMD echo spacing without weakening polling or order."""
    root, mode, reader, digest = xp_evidence
    for iteration, phase in enumerate(["cold", "full", "diff"], 1):
        path = root / (phase + ".log")
        marker = b"XP_CHECK iteration=%d" % iteration
        lines = [
            line + b" " if line == marker else line
            for line in path.read_bytes().splitlines()
        ]
        path.write_bytes(b"\r\n".join(lines) + b"\r\n")
    if change is not None:
        path = root / "cold.log"
        raw = path.read_bytes()
        marker = b"XP_CHECK iteration=1 \r\n"
        if change == "two-spaces":
            raw = raw.replace(marker, b"XP_CHECK iteration=1  \r\n")
        elif change == "tab":
            raw = raw.replace(marker, b"XP_CHECK iteration=1\t\r\n")
        elif change == "text":
            raw = raw.replace(marker, b"XP_CHECK iteration=1 extra\r\n")
        elif change == "leading":
            raw = raw.replace(marker, b" " + marker)
        elif change == "duplicate":
            raw += marker
        else:
            raw = marker + raw.replace(marker, b"")
        path.write_bytes(raw)
        with pytest.raises(ValueError, match="XP polling must precede its TAP pass"):
            VERIFY["verify_xp"](root, mode, reader, digest)
    else:
        result = VERIFY["verify_xp"](root, mode, reader, digest)
        assert result["reads"] == 6
        assert result["generation_ids"] == IDS


@pytest.mark.parametrize(
    "change",
    [
        "missing",
        "duplicate",
        "unequal",
        "zero",
        "stale",
        "error",
        "order",
        "watermark",
        "before-hash",
        "after-hash",
    ],
)
def test_xp_polling_rejects_invalid_evidence(xp_evidence, change):
    """Exercise the actual public acceptance boundary for both XP transports."""
    root, mode, reader, digest = xp_evidence
    path = root / ("cold.log" if mode == "tap" else "guest-transcript.txt")
    content = path.read_text()
    if change == "missing":
        content = "\n".join(content.splitlines()[1:]) + "\n"
    elif change == "duplicate":
        content += content.splitlines()[0] + "\n"
    elif change == "unequal":
        content = content.replace(f"read=2 value={IDS[0]}", f"read=2 value={IDS[1]}", 1)
    elif change == "zero":
        content = content.replace(IDS[0], "0" * 32)
    elif change == "stale":
        target = root / ("full.log" if mode == "tap" else "guest-transcript.txt")
        if mode == "connect":
            content = content.replace(IDS[1], IDS[0])
        else:
            target.write_text(target.read_text().replace(IDS[1], IDS[0]))
    elif change == "error":
        content += "FAIL VMGENID DeviceIoControl win32=5\n"
    elif change == "order":
        lines = content.splitlines()
        lines.insert(0, lines.pop(4))
        content = "\n".join(lines) + "\n"
    elif change == "watermark":
        target = root / "stages.log"
        target.write_text(
            target.read_text().replace(
                "phase=full watermark=2", "phase=full watermark=99"
            )
        )
    elif change == "before-hash":
        (root / "generation-reader-before.json").write_text("{}")
    elif change == "after-hash":
        reader.write_bytes(b"changed XP reader")
    path.write_text(content)
    with pytest.raises(ValueError):
        VERIFY["verify_xp"](root, mode, reader, digest)


@pytest.mark.parametrize("expected", ["", "bad", "0" * 64])
def test_generation_reader_hash_is_enforced(tmp_path, expected):
    """A supplied digest is checked against bytes, never echoed as proof."""
    reader = tmp_path / "reader.exe"
    reader.write_bytes(b"reader")
    with pytest.raises(ValueError):
        VERIFY["check_reader"](reader, expected)


@pytest.fixture
def reader_guard(tmp_path):
    """Use the existing mount-helper fixture pattern without root or a VM."""
    tools = tmp_path / "bin"
    tools.mkdir()
    calls = tmp_path / "calls"
    reader = b"pinned XP reader"
    environment = {
        **os.environ,
        "PATH": f"{tools}:{os.environ['PATH']}",
        "CALLS": str(calls),
        "FIXTURE": str(tmp_path),
    }
    scripts = {
        "disk-guard": '#!/bin/bash\necho disk-closed >> "$CALLS"\n',
        "losetup": """#!/bin/bash
if [[ $1 = --list ]]; then
    if [[ -f $FIXTURE/loop ]]; then
        /bin/cat "$FIXTURE/loop"
        if [[ ${SCENARIO:-} = deferred-detach && -e $FIXTURE/detached ]]; then rm "$FIXTURE/loop"; fi
    fi
    exit 0
fi
if [[ $1 = -d ]]; then
    echo detach >> "$CALLS"
    if [[ ${SCENARIO:-} = deferred-detach || ${SCENARIO:-} = detach-timeout ]]; then
        touch "$FIXTURE/detached"
    else rm "$FIXTURE/loop"; fi
    exit 0
fi
echo attach >> "$CALLS"
printf '%s\\n' "$*" > "$FIXTURE/loop-args"
echo /dev/loop41 > "$FIXTURE/loop"
if [[ ${SCENARIO:-} = allocation-ambiguous ]]; then
    echo /dev/loop42 >> "$FIXTURE/loop"
fi
if [[ ${SCENARIO:-} = allocation-failure ]]; then exit 42; fi
if [[ ${SCENARIO:-} = allocation-signal ]]; then kill -TERM "$PPID"; exit 42; fi
echo /dev/loop41
""",
        "flock": """#!/bin/bash
if [[ ${SCENARIO:-} = lock-failure ]]; then echo lock-failed >> "$CALLS"; exit 1; fi
""",
        "stat": '#!/bin/bash\n[[ $1 = -Lc && $2 = "%d:%i" ]] || exit 97\necho 1:2\n',
        "cat": """#!/bin/bash
case "$1" in
    /sys/block/loop41/loop/backing_file) echo "$FIXTURE/windows.raw" ;;
    /sys/block/loop41/ro) echo 1 ;;
    /sys/block/loop41/loop/offset) echo 32256 ;;
    /sys/block/loop41/loop/sizelimit) echo 0 ;;
    /sys/block/loop41/loop/partscan) echo 0 ;;
    *) exec /bin/cat "$@" ;;
esac
""",
        "ntfs-3g": """#!/bin/bash
mount=${!#}
echo mount >> "$CALLS"
touch "$mount/.mounted"
if [[ ${SCENARIO:-} = mount-failure ]]; then exit 42; fi
if [[ ${SCENARIO:-} = signal ]]; then kill -TERM "$PPID"; exit 0; fi
if [[ ${SCENARIO:-} = parent-signal ]]; then
    parent=$(ps -o ppid= -p "$PPID" | tr -d ' ')
    kill -TERM "$parent"
fi
printf 'pinned XP reader' > "$mount/restore-probe.exe"
printf 'pinned XP reader' > "$mount/xp-connect.exe"
""",
        "mountpoint": """#!/bin/bash
if [[ ${SCENARIO:-} = query-before ]]; then exit 1; fi
if [[ ${SCENARIO:-} = query-cleanup && -f $FIXTURE/loop ]]; then exit 1; fi
if [[ -f $2/.mounted ]]; then exit 0; fi
exit 32
""",
        "sleep": "#!/bin/bash\nexit 0\n",
        "umount": '#!/bin/bash\necho unmount >> "$CALLS"\nrm -f "$1/.mounted" "$1/restore-probe.exe" "$1/xp-connect.exe"\n',
        "sudo": '#!/bin/bash\nexec "$@"\n',
        "never-start": '#!/bin/bash\necho LIVE-HELPER >> "$CALLS"\nexit 99\n',
    }
    for name, text in scripts.items():
        path = tools / name
        path.write_text(text)
        path.chmod(0o700)
    disk = tmp_path / "windows.raw"
    disk.write_bytes(b"OS disk fixture")
    data = tmp_path / "data.raw"
    data.write_bytes(b"different data drive")
    config = tmp_path / "config.json"
    config.write_text(
        json.dumps(
            {
                "drives": [
                    {"path_on_host": str(data), "is_read_only": False},
                    {"path_on_host": str(disk), "is_read_only": False},
                ]
            }
        )
    )
    environment["WINDOWS_DISK_GUARD"] = str(tools / "disk-guard")
    return tmp_path, config, disk, hashlib.sha256(reader).hexdigest(), environment


@pytest.mark.parametrize(
    "scenario,success",
    [
        ("success", True),
        ("bad-hash", False),
        ("mount-failure", False),
        ("signal", False),
        ("parent-signal", False),
        ("query-before", False),
        ("query-cleanup", False),
        ("deferred-detach", True),
        ("detach-timeout", False),
        ("allocation-failure", False),
        ("allocation-signal", False),
        ("allocation-ambiguous", False),
        ("lock-failure", False),
        ("wrong-config", False),
        ("wrong-reader", False),
        ("wrong-offset", False),
        ("readonly-drive", False),
        ("duplicate-drive", False),
    ],
)
def test_xp_reader_guard_preserves_cleanup(reader_guard, scenario, success):
    """The real guard closes its read-only mount/loop on success and failure."""
    root, config, disk, digest, environment = reader_guard
    guard = VERIFIER.parent / "check-reader.sh"
    environment["SCENARIO"] = scenario
    reader = "restore-probe.exe"
    offset = "32256"
    if scenario == "bad-hash":
        digest = "0" * 64
    elif scenario == "wrong-config":
        config.write_text('{"drives": []}')
    elif scenario == "wrong-reader":
        reader = "../unexpected.exe"
    elif scenario == "wrong-offset":
        offset = "not-an-offset"
    elif scenario in ("readonly-drive", "duplicate-drive"):
        selected = {
            "path_on_host": str(disk),
            "is_read_only": scenario == "readonly-drive",
        }
        config.write_text(
            json.dumps(
                {"drives": [selected] * (2 if scenario == "duplicate-drive" else 1)}
            )
        )
    result = subprocess.run(
        ["bash", str(guard), str(config), str(disk), offset, reader, digest],
        env=environment,
        capture_output=True,
        timeout=10,
    )
    assert (result.returncode == 0) == success, result.stderr
    calls = (
        (root / "calls").read_text().splitlines() if (root / "calls").exists() else []
    )
    if "attach" in calls:
        if scenario == "allocation-ambiguous":
            assert calls == ["disk-closed", "attach"]
            assert (root / "loop").read_text().splitlines() == [
                "/dev/loop41",
                "/dev/loop42",
            ]
        elif scenario == "query-cleanup":
            assert calls[-2:] == ["mount", "detach"]
            assert not (root / "loop").exists()
        elif "mount" in calls:
            assert calls[-2:] == ["unmount", "detach"]
            assert (root / "loop").exists() == (scenario == "detach-timeout")
        else:
            assert calls[-1] == "detach"
            assert not (root / "loop").exists()
        assert "--read-only --offset 32256" in (root / "loop-args").read_text()
    if scenario in ("allocation-failure", "allocation-signal"):
        assert calls == ["disk-closed", "attach", "detach"]
        assert result.returncode == (42 if scenario == "allocation-failure" else 143)
    elif scenario == "lock-failure":
        assert calls == ["disk-closed", "lock-failed"]
    directories = list(root.glob(".generation-reader.*"))
    if scenario == "query-cleanup":
        assert len(directories) == 1
        assert (directories[0] / ".mounted").is_file()
    else:
        assert not directories


@pytest.mark.parametrize("mode", ["tap", "connect"])
def test_xp_controller_checks_frozen_reader_before_live_helpers(reader_guard, mode):
    """Direct public invocation cannot bypass a bad reader hash."""
    root, _, disk, _, environment = reader_guard
    windows = VERIFIER.parent.parent
    controller = windows / (
        "egress/tap-check.sh" if mode == "tap" else "legacy-vsock/check.sh"
    )
    executable = root / "bin/never-start"
    environment.update(
        {
            "WINDOWS_SECURE_HELPER": str(windows / "egress/secure-state.sh"),
            "WINDOWS_GENERATION_CHECKS": "1",
            "WINDOWS_GENERATION_PROBE_SHA256": "0" * 64,
            "WINDOWS_CHECK_BASE": str(root),
            "WINDOWS_CHECK_FC": str(executable),
            "WINDOWS_CHECK_SNAPSHOT_EDITOR": str(executable),
            "WINDOWS_CHECK_SERVER": str(executable),
            "WINDOWS_CHECK_NAMESPACE": "fixture",
            "WINDOWS_CHECK_ADDRESS": "192.0.2.1",
            "WINDOWS_GUEST_TAG": "XP",
            "WINDOWS_PARTITION_OFFSET": "32256",
            "WINDOWS_CHECK_DISK": str(disk),
            "WINDOWS_CONNECT_BASE": str(root),
            "WINDOWS_CONNECT_FC": str(executable),
            "WINDOWS_CONNECT_PROXY": str(executable),
            "WINDOWS_CONNECT_SIDECAR": str(executable),
            "WINDOWS_CONNECT_SNAPSHOT_EDITOR": str(executable),
            "WINDOWS_CONNECT_TAG": "XPLEGACY",
            "WINDOWS_CONNECT_OFFSET": "32256",
            "WINDOWS_CONNECT_DISK": str(disk),
        }
    )
    result = subprocess.run(
        ["bash", str(controller)], env=environment, capture_output=True, timeout=10
    )
    assert result.returncode != 0
    assert (root / "calls").exists(), result.stderr
    calls = (root / "calls").read_text()
    assert "attach" in calls and "detach" in calls and "LIVE-HELPER" not in calls
    assert (
        not (root / "run1/proxy-input").exists()
        and not (root / "run/proxy-input").exists()
    )
    frozen = root / ("run1" if mode == "tap" else "run")
    for name, source in [
        ("check-reader.sh", windows / "generation/check-reader.sh"),
        ("verify-generation.py", VERIFIER),
        ("mount-state.sh", windows / "egress/mount-state.sh"),
    ]:
        assert (frozen / name).read_bytes() == source.read_bytes()
        assert (frozen / name).stat().st_mode & 0o777 == 0o400


@pytest.mark.parametrize("shape", ["missing", "symlink", "directory"])
def test_generation_reader_requires_canonical_regular_file(tmp_path, shape):
    """An executable hash cannot authorise a substituted path or missing reader."""
    reader = tmp_path / "reader.exe"
    if shape == "symlink":
        target = tmp_path / "other.exe"
        target.write_bytes(b"reader")
        reader.symlink_to(target)
    elif shape == "directory":
        reader.mkdir()
    with pytest.raises((OSError, ValueError)):
        VERIFY["check_reader"](reader, hashlib.sha256(b"reader").hexdigest())


@pytest.mark.parametrize("mode", ["tap", "connect"])
def test_offline_generation_option_cannot_silently_downgrade(reader_guard, mode):
    """Reject mistyped opt-in before disk inspection or mount."""
    root, _, _, _, environment = reader_guard
    windows = VERIFIER.parent.parent
    environment.update(
        {
            "WINDOWS_CHECK_BASE": str(root),
            "WINDOWS_PARTITION_OFFSET": "32256",
            "WINDOWS_CONNECT_BASE": str(root),
            "WINDOWS_CONNECT_PREFIX": "xp-legacy",
            "WINDOWS_CONNECT_TAG": "XPLEGACY",
            "WINDOWS_GENERATION_CHECKS": "true",
        }
    )
    script = windows / f"egress/{mode}-verify.sh"
    result = subprocess.run(
        ["bash", str(script)], env=environment, capture_output=True, timeout=10
    )
    assert result.returncode != 0
    assert b"WINDOWS_GENERATION_CHECKS must be 0 or 1" in result.stderr
    assert not (root / "calls").exists()


@pytest.fixture
def windows_cleanup():
    """Read actual shell cleanup functions without starting their controllers."""
    windows = VERIFIER.parent.parent
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


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "failure,code",
    [
        ("none", 0),
        ("stop", 7),
        ("secure", 11),
        ("discard-files", 17),
        ("discard-tree", 18),
        ("fifo", 19),
        ("wait", 23),
        ("group", 29),
        ("remove", 31),
        ("all", 7),
    ],
)
def test_connect_cleanup_preserves_error_and_attempts_all(
    windows_cleanup, original, failure, code, tmp_path
):
    """A cleanup failure must not replace an original exit or skip later work."""
    statuses = {
        "stop": 7,
        "secure": 11,
        "discard-files": 17,
        "discard-tree": 18,
        "fifo": 19,
        "wait": 23,
        "group": 29,
        "remove": 31,
    }
    values = {
        key: value if failure in [key, "all"] else 0 for key, value in statuses.items()
    }
    if failure.startswith("discard") and original == 0:
        values["stop"] = 7
        code = 7
    script = f"""set -euo pipefail
stop_vm() {{ echo stop; return {values['stop']}; }}
secure_stop() {{ echo secure; return {values['secure']}; }}
parent_gone=0
group_gone=0
wait_for_exit() {{ echo wait; parent_gone=1; return {values['wait']}; }}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return "$parent_gone"; fi
 echo group-${{1#-}}
 if [[ {values['group']} == 0 ]]; then group_gone=1; fi
 return {values['group']}
}}
printf() {{
 if [[ $1 == '\\n' ]]; then echo fifo >&4; return {values['fifo']}; fi
 builtin printf "$@"
}}
rm() {{
 if [[ ${{2:-}} == run/proxy-input ]]; then echo remove; return {values['remove']}; fi
 if [[ $1 == -f ]]; then echo discard-files; return {values['discard-files']}; fi
 echo discard-tree; return {values['discard-tree']}
}}
proxy_child=47
secure_current=state
secure_pids=()
exec 4>&1
exec 3>/dev/null
{windows_cleanup['secure_discard']}
{windows_cleanup.get('cleanup_failed', '')}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (original or code), result.stderr
    calls = result.stdout.decode().splitlines()
    discard = original != 0 or values["stop"] != 0 or values["secure"] != 0
    expected = ["stop", "secure"]
    if discard:
        expected += ["discard-files", "discard-tree"]
    expected += ["fifo", "wait", "group-TERM"]
    expected += ["group-KILL"] if values["group"] else ["remove"]
    assert calls == expected
    if failure != "none":
        assert b"CONNECT cleanup failed:" in result.stderr


@pytest.mark.parametrize("files,tree", [(0, 0), (7, 0), (0, 11), (7, 11)])
def test_secure_discard_keeps_first_failure(windows_cleanup, files, tree, tmp_path):
    """Both actual discard command positions run and the first failure survives."""
    script = f"""set -euo pipefail
rm() {{
 if [[ $1 == -f ]]; then echo files; return {files}; fi
 echo tree; return {tree}
}}
secure_current=state
secure_pids=()
{windows_cleanup['secure_discard']}
result=0
secure_discard || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (files or tree)
    assert result.stdout.splitlines() == [b"files", b"tree"]


@pytest.mark.parametrize("remove,unlink", [(0, 0), (7, 0), (0, 11), (7, 11)])
def test_connect_socket_cleanup_keeps_first_failure(
    windows_cleanup, remove, unlink, tmp_path
):
    """A later successful unlink must not hide the first socket removal error."""
    with socket.socket(socket.AF_UNIX) as console:
        with contextlib.chdir(tmp_path):
            console.bind("console.sock")
        script = f"""set -euo pipefail
rm() {{ echo remove; return {remove}; }}
unlink() {{ echo unlink; return {unlink}; }}
{windows_cleanup['remove_sockets']}
result=0
remove_sockets || result=$?
exit "$result"
"""
        result = subprocess.run(
            ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
        )
    assert result.returncode == (remove or unlink)
    assert result.stdout.splitlines() == [b"remove", b"unlink"]


@pytest.mark.parametrize(
    "term,wait,sockets",
    [(0, 0, 0), (7, 0, 0), (0, 143, 0), (0, 137, 0), (0, 0, 11), (7, 137, 11)],
)
def test_connect_stop_keeps_first_failure(
    windows_cleanup, term, wait, sockets, tmp_path
):
    """Stopping a VM retains TERM/wait/socket errors and attempts every step."""
    script = f"""set -euo pipefail
gone=0
kill() {{ if [[ $1 == -0 ]]; then return "$gone"; fi; echo term; return {term}; }}
wait_for_exit() {{ echo wait; gone=1; return {wait}; }}
remove_sockets() {{ echo sockets; return {sockets}; }}
child=41
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (
        term or (wait if wait not in [0, 143] else 0) or sockets
    )
    assert result.stdout.splitlines() == [b"term", b"wait", b"sockets"]


@pytest.mark.parametrize("kill_result,still_alive", [(0, True), (7, True), (7, False)])
def test_connect_wait_reports_failed_kill_of_live_child(
    windows_cleanup, kill_result, still_alive, tmp_path
):
    """A failed forced kill is an error while the child is still observed live."""
    script = f"""set -euo pipefail
forced=0
kill() {{
 if [[ $1 == -KILL ]]; then forced=1; echo kill; return {kill_result}; fi
 if [[ $forced == 1 ]]; then return {0 if still_alive else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo wait; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
result=0
wait_for_exit 41 || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    expected = (kill_result or 1) if still_alive else 0
    assert result.returncode == expected
    assert result.stdout.splitlines() == ([b"kill"] if expected else [b"kill", b"wait"])


@pytest.mark.parametrize("still_alive", [False, True])
def test_connect_stop_allows_natural_exit_during_term(
    windows_cleanup, still_alive, tmp_path
):
    """A TERM failure is benign only when the owned VMM is then observed gone."""
    script = f"""set -euo pipefail
signalled=0
reaped=0
kill() {{
 if [[ $1 == -TERM ]]; then signalled=1; echo term; return 7; fi
 if [[ $reaped == 1 ]]; then return 1; fi
 if [[ $signalled == 1 ]]; then return {0 if still_alive else 1}; fi
 return 0
}}
wait_for_exit() {{ echo wait; reaped=1; return 0; }}
remove_sockets() {{ echo sockets; return 0; }}
child=41
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (7 if still_alive else 0)
    assert result.stdout.splitlines() == [b"term", b"wait", b"sockets"]


@pytest.mark.parametrize("disappearance", ["before", "during", "never"])
def test_connect_cleanup_checks_group_signal_races(
    windows_cleanup, disappearance, tmp_path
):
    """A signal failure must not fail cleanup after the owned group disappears."""
    script = f"""set -euo pipefail
stop_vm() {{ :; }}
secure_stop() {{ :; }}
secure_discard() {{ :; }}
wait_for_exit() {{ :; }}
rm() {{ echo remove; }}
group_gone={1 if disappearance == 'before' else 0}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return 1; fi
 echo signal-${{1#-}}
 {'group_gone=1' if disappearance == 'during' else ':'}
 return 7
}}
proxy_child=47
{windows_cleanup.get('cleanup_failed', '')}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit 0
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (7 if disappearance == "never" else 0)
    expected = [] if disappearance == "before" else [b"signal-TERM"]
    if disappearance == "never":
        expected += [b"signal-KILL"]
    if disappearance != "never":
        expected += [b"remove"]
    assert result.stdout.splitlines() == expected
    assert (b"CONNECT cleanup failed:" in result.stderr) == (disappearance == "never")


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "mode,code",
    [
        ("before", 0),
        ("natural", 0),
        ("forced-gone", 1),
        ("forced-live", 1),
        ("term-race", 0),
        ("kill-race", 0),
        ("term-error", 11),
        ("kill-error", 7),
        ("parent-live", 7),
    ],
)
def test_connect_cleanup_requires_proxy_group_stop(
    windows_cleanup, original, mode, code, tmp_path
):
    """Forced group stops fail; live parents or groups retain their input FIFO."""
    script = f"""set -euo pipefail
stop_vm() {{ :; }}
secure_stop() {{ :; }}
secure_discard() {{ :; }}
wait_for_exit() {{ return {7 if mode == 'parent-live' else 0}; }}
rm() {{ echo remove; }}
group_gone={1 if mode in ['before', 'parent-live'] else 0}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return {0 if mode == 'parent-live' else 1}; fi
 echo signal-${{1#-}}
 if [[ $1 == -TERM ]]; then
  if [[ {mode} == natural || {mode} == term-race ]]; then group_gone=1; fi
  return {11 if mode == 'term-error' else 7 if mode == 'term-race' else 0}
 fi
 if [[ {mode} != forced-live && {mode} != kill-error ]]; then group_gone=1; fi
 return {7 if mode in ['kill-race', 'kill-error'] else 0}
}}
proxy_child=47
exec 3>/dev/null
{windows_cleanup['cleanup_failed']}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (original or code), result.stderr
    calls = []
    if mode not in ["before", "parent-live"]:
        calls.append(b"signal-TERM")
    if mode in ["forced-gone", "forced-live", "kill-race", "term-error", "kill-error"]:
        calls.append(b"signal-KILL")
    if mode not in ["forced-live", "kill-error", "parent-live"]:
        calls.append(b"remove")
    assert result.stdout.splitlines() == calls
    assert (b"CONNECT cleanup failed:" in result.stderr) == (code != 0)


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "failure,code",
    [
        ("none", 0),
        ("stop", 7),
        ("secure", 11),
        ("files", 17),
        ("tree", 18),
        ("term", 19),
        ("kill", 23),
        ("wait", 29),
        ("all", 7),
    ],
)
def test_tap_cleanup_preserves_error_and_attempts_all(
    windows_cleanup, original, failure, code, tmp_path
):
    """Every owned cleanup step runs without replacing the primary failure."""
    values = {
        key: value if failure in [key, "all"] else 0
        for key, value in dict(
            stop=7, secure=11, files=17, tree=18, term=19, kill=23, wait=29
        ).items()
    }
    if failure in ["files", "tree"] and original == 0:
        values["stop"] = code = 7
    forced = failure in ["kill", "all"]
    script = f"""set -euo pipefail
stop() {{ echo stop; return {values['stop']}; }}
secure_stop() {{ echo secure; return {values['secure']}; }}
kill() {{
 if [[ $1 == -0 ]]; then return 0; fi
 echo echo-${{1#-}}
 if [[ $1 == -TERM ]]; then return {values['term']}; fi
 return {values['kill']}
}}
polls=0
secure_wait_stopped() {{
 polls=$((polls+1))
 if [[ $polls == 1 ]]; then return {1 if forced else 0}; fi
 return 0
}}
wait() {{ echo echo-wait; return {values['wait']}; }}
rm() {{
 if [[ $1 == -f ]]; then echo files; return {values['files']}; fi
 echo tree; return {values['tree']}
}}
echo_child=47
secure_pids=()
secure_current=state
{windows_cleanup['secure_discard']}
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (original or code), result.stderr
    expected = [b"stop", b"secure"]
    if original or values["stop"] or values["secure"]:
        expected += [b"files", b"tree"]
    expected += [b"echo-TERM"]
    if forced:
        expected += [b"echo-KILL"]
    assert result.stdout.splitlines() == [*expected, b"echo-wait"]
    assert (b"TAP cleanup failed:" in result.stderr) == (failure != "none")


@pytest.mark.parametrize(
    "term,wait,sockets,expected",
    [
        (0, 0, 0, 0),
        (7, 0, 0, 7),
        (0, 143, 0, 0),
        (0, 137, 0, 137),
        (0, 0, 11, 11),
        (7, 137, 11, 7),
    ],
)
def test_tap_stop_keeps_first_failure(
    windows_cleanup, term, wait, sockets, expected, tmp_path
):
    """Signal the root VMM, wait for its wrapper and attempt socket cleanup."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
gone=0
kill() {{ return "$gone"; }}
secure_wait_stopped() {{ gone=1; return 0; }}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == kill ]]; then echo "$2-$3"; return {term}; fi
 echo sockets; return {sockets}
}}
wait() {{ echo wait-$1; return {wait}; }}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
echo child="$child"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == expected
    assert result.stdout.splitlines() == [
        b"-TERM-61",
        b"wait-41",
        b"sockets",
        b"child=",
    ]


@pytest.mark.parametrize(
    "kill_result,still_live", [(0, False), (7, False), (7, True), (0, True)]
)
def test_tap_stop_bounds_forced_kill(
    windows_cleanup, kill_result, still_live, tmp_path
):
    """Do not wait for a live wrapper; retain ownership and strict forced-stop failure."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
forced=0
kill() {{ if [[ $forced == 0 ]]; then return 0; fi; return {0 if still_live else 1}; }}
sleep() {{ :; }}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 echo "$2-$3"
 if [[ $2 == -KILL ]]; then forced=1; return {kill_result}; fi
 return 0
}}
wait() {{ echo wait-$1; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
echo child="$child"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (kill_result if still_live and kill_result else 1)
    expected = [b"-TERM-61", b"-KILL-61"]
    if not still_live:
        expected += [b"wait-41", b"sockets"]
    assert result.stdout.splitlines() == [
        *expected,
        b"child=41" if still_live else b"child=",
    ]


@pytest.mark.parametrize(
    "pid,read_error", [("61", 7), ("-1", 0), ("0", 0), ("61 62", 0), ("", 0)]
)
def test_tap_stop_rejects_invalid_root_pid(windows_cleanup, pid, read_error, tmp_path):
    """A failed PID read or invalid scalar must never become a root signal target."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
kill() {{ return 0; }}
sleep() {{ :; }}
sudo() {{
 if [[ $1 == cat ]]; then echo '{pid}'; return {read_error}; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 echo unexpected-signal; return 0
}}
wait() {{ echo unexpected-wait; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (read_error or 1)
    assert result.stdout.splitlines() == []


@pytest.mark.parametrize(
    "mode",
    [
        "natural",
        "term-gone",
        "term-live",
        "kill-gone",
        "kill-failed-gone",
        "kill-live",
        "wait",
    ],
)
def test_secure_stop_retains_live_ownership(windows_cleanup, mode, tmp_path):
    """Attempt both helpers, retain live PIDs and refuse to discard their inputs."""
    forced = mode in ["term-live", "kill-gone", "kill-failed-gone", "kill-live"]
    script = f"""set -euo pipefail
secure_pids=(51 52)
secure_current=state
forced51=0
forced52=0
kill() {{
 local name=forced${{2}}
 if [[ $1 == -TERM ]]; then
  echo term-$2
  return {7 if mode.startswith('term-') else 0}
 fi
 if [[ $1 == -KILL ]]; then
  echo kill-$2
  printf -v "$name" %s 1
  return {11 if mode in ['kill-failed-gone', 'kill-live'] else 0}
 fi
 if [[ ${{!name}} == 1 ]]; then return {0 if mode == 'kill-live' else 1}; fi
 return {0 if forced else 1}
}}
sleep() {{ :; }}
wait() {{ echo wait-$1; return {17 if mode == 'wait' else 0}; }}
rm() {{ echo discard; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['secure_stop']}
{windows_cleanup['secure_discard']}
result=0
secure_stop || result=$?
echo remaining="${{secure_pids[*]}}"
discard=0
secure_discard || discard=$?
echo discard-result="$discard"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    expected_result = {
        "term-live": 7,
        "kill-gone": 1,
        "kill-failed-gone": 1,
        "kill-live": 11,
        "wait": 17,
    }.get(mode, 0)
    assert result.returncode == expected_result, result.stderr
    expected = [b"term-51", b"term-52"]
    for pid in [51, 52]:
        if forced:
            expected += [f"kill-{pid}".encode()]
        if mode != "kill-live":
            expected += [f"wait-{pid}".encode()]
    expected += [b"remaining=51 52"] if mode == "kill-live" else [b"remaining="]
    if mode != "kill-live":
        expected += [b"discard", b"discard"]
    expected += [b"discard-result=1" if mode == "kill-live" else b"discard-result=0"]
    assert result.stdout.splitlines() == expected
    assert (b"Refusing to discard live" in result.stderr) == (mode == "kill-live")


@pytest.mark.parametrize("live", [False, True])
def test_secure_wait_is_bounded(windows_cleanup, live, tmp_path):
    """The shared poll has a fixed bound and never calls the blocking wait builtin."""
    script = f"""set -euo pipefail
probes=0
sleeps=0
kill() {{ probes=$((probes+1)); return {0 if live else 1}; }}
sleep() {{ sleeps=$((sleeps+1)); }}
wait() {{ echo unexpected-wait; }}
{windows_cleanup['secure_wait_stopped']}
result=0
secure_wait_stopped 51 || result=$?
echo "$probes $sleeps"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == int(live)
    assert result.stdout.splitlines() == ([b"101 100"] if live else [b"1 0"])


@pytest.mark.parametrize(
    "term_error,kill_error,still_live",
    [(0, 0, False), (0, 7, False), (0, 7, True), (0, 0, True), (11, 7, True)],
)
def test_tap_echo_stop_does_not_wait_for_live_child(
    windows_cleanup, term_error, kill_error, still_live, tmp_path
):
    """Echo cleanup is bounded even after both signals fail and preserves its PID."""
    script = f"""set -euo pipefail
stop() {{ :; }}
secure_stop() {{ :; }}
secure_discard() {{ :; }}
echo_child=47
forced=0
kill() {{
 if [[ $1 == -TERM ]]; then echo term; return {term_error}; fi
 if [[ $1 == -KILL ]]; then forced=1; echo kill; return {kill_error}; fi
 if [[ $forced == 1 ]]; then return {0 if still_live else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo wait; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
exit() {{ echo child="$echo_child"; builtin exit "$1"; }}
trap cleanup EXIT
exit 0
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (term_error or (kill_error if still_live else 0) or 1)
    expected = [b"child=47", b"term", b"kill"]
    if not still_live:
        expected += [b"wait"]
    assert result.stdout.splitlines() == [
        *expected,
        b"child=47" if still_live else b"child=",
    ]


@pytest.mark.parametrize("controller", ["tap", "echo"])
def test_tap_stop_allows_natural_exit_during_term(
    windows_cleanup, controller, tmp_path
):
    """An observed-gone signal race is benign for both the VMM and echo child."""
    script = f"""set -euo pipefail
child=41
echo_child=47
run=fixture
base=fixture
gone=0
kill() {{
 if [[ $1 == -0 ]]; then return "$gone"; fi
 echo term; gone=1; return 7
}}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 kill "$2" "$3"
}}
wait() {{ echo wait-$1; return 143; }}
{windows_cleanup['secure_wait_stopped']}
"""
    if controller == "tap":
        script += f"""{windows_cleanup['tap_stop']}
result=0
stop || result=$?
exit "$result"
"""
    else:
        script += f"""stop() {{ :; }}
secure_stop() {{ :; }}
secure_discard() {{ :; }}
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit 0
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == (
        [b"term", b"wait-41", b"sockets"]
        if controller == "tap"
        else [b"term", b"wait-47"]
    )


@pytest.mark.parametrize("live", [False, True])
def test_secure_discard_checks_retained_helper(windows_cleanup, live, tmp_path):
    """Only observed live helpers prevent discard; a later natural exit is safe."""
    script = f"""set -euo pipefail
secure_pids=(51)
secure_current=state
kill() {{ return {0 if live else 1}; }}
rm() {{ echo discard; }}
{windows_cleanup['secure_discard']}
result=0
secure_discard || result=$?
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == int(live)
    assert result.stdout.splitlines() == ([] if live else [b"discard", b"discard"])


@pytest.mark.parametrize("original", [0, 42, 130, 143])
def test_tap_cleanup_survives_failed_diagnostics(windows_cleanup, original, tmp_path):
    """A failed stderr write must not abort EXIT cleanup or replace its primary exit."""
    script = f"""set -euo pipefail
stop() {{ echo stop; return 7; }}
secure_stop() {{ echo secure; return 11; }}
secure_discard() {{ echo discard; return 17; }}
secure_wait_stopped() {{ return 0; }}
kill() {{ if [[ $1 == -0 ]]; then return 0; fi; echo echo-TERM; return 19; }}
wait() {{ echo echo-wait; return 23; }}
printf() {{ return 31; }}
echo_child=47
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (original or 7)
    assert result.stdout.splitlines() == [
        b"stop",
        b"secure",
        b"discard",
        b"echo-TERM",
        b"echo-wait",
    ]


@pytest.mark.parametrize("operation", ["stop", "discard", "vmm"])
def test_windows_cleanup_survives_failed_diagnostics(
    windows_cleanup, operation, tmp_path
):
    """A helper failure keeps its status and remaining cleanup even without stderr."""
    script = f"""set -euo pipefail
secure_pids=(51 52)
secure_current=state
child=41
run=fixture
base=fixture
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == kill ]]; then kill "$2" "$3"; return $?; fi
 echo unexpected-socket-removal
}}
kill() {{
 if [[ $1 == -0 ]]; then return 0; fi
 echo "$1-$2"
 if [[ $1 == -TERM ]]; then return 7; fi
 return 11
}}
sleep() {{ :; }}
printf() {{ return 31; }}
wait() {{ echo unexpected-wait; }}
rm() {{ echo unexpected-discard; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['secure_stop']}
{windows_cleanup['secure_discard']}
{windows_cleanup['tap_stop']}
trap 'code=$?; echo "result=$code remaining=${{secure_pids[*]}}"' EXIT
{'stop' if operation == 'vmm' else 'secure_' + operation}
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    if operation == "stop":
        assert result.returncode == 7
        assert result.stdout.splitlines() == [
            b"-TERM-51",
            b"-TERM-52",
            b"-KILL-51",
            b"-KILL-52",
            b"result=7 remaining=51 52",
        ]
    elif operation == "vmm":
        assert result.returncode == 7
        assert result.stdout.splitlines() == [
            b"-TERM-61",
            b"-KILL-61",
            b"result=7 remaining=51 52",
        ]
    else:
        assert result.returncode == 1
        assert result.stdout.splitlines() == [b"result=1 remaining=51 52"]


@pytest.mark.parametrize("kill_result", [0, 7])
@pytest.mark.parametrize(
    "term,still_live", [(0, False), (0, True), (11, False), (11, True)]
)
def test_connect_stop_retains_live_ownership(
    windows_cleanup, term, still_live, kill_result, tmp_path
):
    """A failed actual forced kill must retain the live child and its sockets."""
    script = f"""set -euo pipefail
child=41
forced=0
kill() {{
 if [[ $1 == -TERM ]]; then echo TERM-$2; return {term}; fi
 if [[ $1 == -KILL ]]; then echo KILL-$2; forced=1; return {kill_result}; fi
 if [[ $forced == 1 ]]; then return {0 if still_live else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo WAIT-$1; return 0; }}
remove_sockets() {{ echo SOCKETS; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
echo "child=$child"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == (term or ((kill_result or 1) if still_live else 0))
    expected = [b"TERM-41", b"KILL-41"]
    if still_live:
        expected += [b"child=41"]
    else:
        expected += [b"WAIT-41", b"SOCKETS", b"child="]
    assert result.stdout.splitlines() == expected


@pytest.mark.parametrize("wait_status,expected", [(0, 1), (143, 1), (7, 7)])
def test_connect_stop_checks_liveness_after_wait(
    windows_cleanup, wait_status, expected, tmp_path
):
    """A wait result alone cannot authorise forgetting an observed live child."""
    script = f"""set -euo pipefail
child=41
kill() {{ return 0; }}
wait_for_exit() {{ return {wait_status}; }}
remove_sockets() {{ echo unexpected-sockets; }}
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
echo "child=$child"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == expected
    assert result.stdout.splitlines() == [b"child=41"]


@pytest.mark.parametrize(
    "mode,kill_status,wait_status,expected,sleeps",
    [
        ("natural", 0, 143, 143, 0),
        ("killed", 0, 137, 137, 100),
        ("failed-gone", 7, 0, 0, 100),
        ("live", 0, 0, 1, 200),
        ("failed-live", 7, 0, 7, 100),
        ("late-exit", 0, 137, 137, 103),
    ],
)
def test_connect_wait_observes_bounded_disappearance(
    windows_cleanup, mode, kill_status, wait_status, expected, sleeps, tmp_path
):
    """Successful KILL is a request; only observed exit permits the blocking wait."""
    script = f"""set -euo pipefail
forced=0
post_probes=0
sleeps=0
kill() {{
 if [[ $1 == -KILL ]]; then forced=1; echo KILL-$2; return {kill_status}; fi
 if [[ $forced == 0 ]]; then return {1 if mode == 'natural' else 0}; fi
 post_probes=$((post_probes+1))
 {'if [[ $post_probes -le 3 ]]; then return 0; fi' if mode == 'late-exit' else ':'}
 return {0 if mode in ['live', 'failed-live'] else 1}
}}
sleep() {{ sleeps=$((sleeps+1)); }}
wait() {{
 if kill -0 "$1"; then echo unexpected-live-wait; return 99; fi
 echo WAIT-$1; return {wait_status}
}}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
result=0
wait_for_exit 41 || result=$?
echo "sleeps=$sleeps"
exit "$result"
"""
    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, capture_output=True, timeout=5
    )
    assert result.returncode == expected
    calls = [] if mode == "natural" else [b"KILL-41"]
    if mode not in ["live", "failed-live"]:
        calls += [b"WAIT-41"]
    assert result.stdout.splitlines() == [*calls, f"sleeps={sleeps}".encode()]
