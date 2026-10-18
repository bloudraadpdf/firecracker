# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check exact prearmed-generation evidence without a Windows guest."""

import hashlib
import json
import os
import runpy
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
        check=False,
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
        ["bash", str(controller)],
        env=environment,
        capture_output=True,
        timeout=10,
        check=False,
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
        ["bash", str(script)],
        env=environment,
        capture_output=True,
        timeout=10,
        check=False,
    )
    assert result.returncode != 0
    assert b"WINDOWS_GENERATION_CHECKS must be 0 or 1" in result.stderr
    assert not (root / "calls").exists()
