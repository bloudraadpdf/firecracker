# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check exact prearmed-generation evidence without a Windows guest."""

import hashlib
import json
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
