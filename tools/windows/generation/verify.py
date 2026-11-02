# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Verify prearmed driver requests against exact guest and host report records."""

import argparse
import hashlib
import json
import os
import re
import stat
from pathlib import Path

EVENT_FILES = {
    "iteration-1.stdout": 4096,
    "iteration-1.stderr": 4096,
    "iteration-2.stdout": 4096,
    "iteration-2.stderr": 4096,
    "reports.log": 16384,
}


def require(condition, message):
    """Reject invalid evidence even when Python optimisation is enabled."""
    if not condition:
        raise ValueError(message)


def read_regular(path, limit):
    """Read one bounded canonical regular file without accepting changed evidence."""
    info = path.lstat()
    require(
        path.resolve() == path and stat.S_ISREG(info.st_mode),
        f"Not a regular evidence file: {path}",
    )
    require(info.st_size <= limit, f"Evidence too large: {path}")
    fields = (
        "st_dev",
        "st_ino",
        "st_mode",
        "st_nlink",
        "st_size",
        "st_mtime_ns",
        "st_ctime_ns",
    )
    before = tuple(getattr(info, name) for name in fields)
    with path.open("rb") as source:
        require(
            tuple(getattr(os.fstat(source.fileno()), name) for name in fields)
            == before,
            f"Evidence changed: {path}",
        )
        raw = source.read(limit + 1)
        require(
            tuple(getattr(os.fstat(source.fileno()), name) for name in fields)
            == before,
            f"Evidence changed: {path}",
        )
    require(
        len(raw) == info.st_size
        and tuple(getattr(path.lstat(), name) for name in fields) == before,
        f"Evidence changed: {path}",
    )
    return raw


def check_reader(path, expected):
    """Bind the executable that makes the inline reads, not a separate probe."""
    require(
        re.fullmatch(r"[0-9a-fA-F]{64}", expected) is not None,
        "Expected the reader SHA256 as 64 hex digits",
    )
    raw = read_regular(path, 16 * 1024 * 1024)
    digest = hashlib.sha256(raw).hexdigest()
    require(digest == expected.lower(), "Unexpected generation reader executable")
    return {"bytes": len(raw), "sha256": digest}


def read_evidence(directory):
    """Capture bounded regular files once for both parsing and hashes."""
    require(
        directory.resolve() == directory and directory.is_dir(),
        "Invalid evidence directory",
    )
    require(
        {path.name for path in directory.iterdir()} == set(EVENT_FILES),
        "Unexpected evidence files",
    )
    contents = {
        name: read_regular(directory / name, limit)
        for name, limit in EVENT_FILES.items()
    }
    return contents


def collect(source, root):
    """Copy only closed bounded evidence from the caller's read-only mount."""
    contents = read_evidence(source)
    target = root / "events"
    target.mkdir(mode=0o700)
    for name, data in contents.items():
        with os.fdopen(
            os.open(target / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb"
        ) as output:
            output.write(data)


def report_view(lines):
    """Separate report text from host receipt timestamps."""
    view, records = [], []
    for index, line in enumerate(lines):
        if line.startswith("report: "):
            match = re.fullmatch(r"(report: .+) host_unix_ms=([0-9]+)", line)
            require(
                match is not None and " host_unix_ms=" not in match[1],
                f"Invalid host receipt timestamp at echo.log line {index + 1}",
            )
            records.append(
                {"raw_index": index, "raw": line, "host_unix_ms": int(match[2])}
            )
            line = match[1]
        view.append(line)
    return view, records


def verify(root, tag):
    """Validate captured generation, reader and transfer evidence."""
    require(re.fullmatch(r"[A-Za-z0-9_-]+", tag), "Invalid guest tag")
    host_raw = (root / "echo.log").read_bytes()
    host, records = report_view(host_raw.decode("utf-8").splitlines())
    contents = read_evidence(root / "events")
    evidence = {
        name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        for name, data in contents.items()
    }
    reports_raw = contents["reports.log"]
    require(reports_raw.endswith(b"\n"), "Incomplete guest reports.log")
    guest = reports_raw.decode("utf-8").split("\n")[:-1]
    require(len(guest) == 7, "Expected exactly seven guest reports in reports.log")
    require(
        guest
        == [
            line.removeprefix("report: ")
            for line in host
            if line.startswith("report: ")
        ],
        "Guest reports.log and host echo.log reports differ",
    )
    require(
        not any(f"FAIL {tag}_NET" in line for line in host + guest),
        f"Guest reported FAIL {tag}_NET",
    )
    passes = {}
    for iteration in [1, 2, 3]:
        pattern = rf"^report: PASS {tag}_NET iteration={iteration} bytes=2097152 DNS HTTPS TLS1.2 TCP_EQUAL STORAGE_SHA256=[0-9a-f]{{64}} VMGENID=([0-9a-f]{{32}}) .+$"
        matches = [
            (index, re.fullmatch(pattern, line))
            for index, line in enumerate(host)
            if re.fullmatch(pattern, line)
        ]
        require(len(matches) == 1, f"Expected one valid PASS for iteration {iteration}")
        index, match = matches[0]
        passes[iteration] = {"index": index, "id": match[1]}
        require(
            guest.count(host[index].removeprefix("report: ")) == 1,
            f"Duplicate guest PASS for iteration {iteration}",
        )
        require(
            sum(
                line.startswith(f"report: PASS {tag}_NET iteration={iteration} ")
                for line in host
            )
            == 1,
            f"Duplicate host PASS for iteration {iteration}",
        )
        require(
            sum(
                line.startswith(f"PASS {tag}_NET iteration={iteration} ")
                for line in guest
            )
            == 1,
            f"Duplicate guest PASS for iteration {iteration}",
        )
    ids = [passes[iteration]["id"] for iteration in [1, 2, 3]]
    require(
        len(set(ids)) == 3 and all(value != "0" * 32 for value in ids),
        "Cold/full/diff generation IDs must be nonzero and distinct",
    )
    armed, complete = {}, {}
    for iteration in [1, 2]:
        pattern = rf"^report: VMGENID_EVENT_ARMED iteration={iteration} pid=([1-9][0-9]*) old=({ids[iteration - 1]}) alive=true timeout_ms=300000$"
        matches = [
            (index, re.fullmatch(pattern, line))
            for index, line in enumerate(host)
            if re.fullmatch(pattern, line)
        ]
        require(
            len(matches) == 1,
            f"Expected one live ARMED request for iteration {iteration}",
        )
        index, match = matches[0]
        marker = host[index].removeprefix("report: ")
        require(
            guest.count(marker) == 1 and index < passes[iteration]["index"],
            f"ARMED request must precede PASS {iteration}",
        )
        armed[iteration] = {"index": index, "pid": match[1], "marker": marker}
        name = f"iteration-{iteration}.stdout"
        output = contents[name].decode("utf-8-sig").splitlines()
        require(
            output
            == [f"ARMED VMGENID={ids[iteration - 1]}", f"VMGENID={ids[iteration]}"],
            f"Invalid pending/completed child output: {name}",
        )
        require(
            contents[f"iteration-{iteration}.stderr"] == b"",
            f"Child stderr is not empty for iteration {iteration}",
        )
        marker = f"VMGENID_EVENT_COMPLETE iteration={iteration + 1} armed_iteration={iteration} pid={match[1]} old={ids[iteration - 1]} new={ids[iteration]} exit=0"
        matches = [
            index for index, line in enumerate(host) if line == "report: " + marker
        ]
        require(
            len(matches) == 1 and guest.count(marker) == 1,
            f"Expected same-PID successful completion for iteration {iteration}",
        )
        complete[iteration + 1] = {
            "index": matches[0],
            "pid": match[1],
            "marker": marker,
        }
        require(
            passes[iteration]["index"] < matches[0] < passes[iteration + 1]["index"],
            f"Completion for iteration {iteration} is outside its restore boundary",
        )
    expected = [
        armed[1]["marker"],
        complete[2]["marker"],
        armed[2]["marker"],
        complete[3]["marker"],
    ]
    require(
        [
            line.removeprefix("report: ")
            for line in host
            if line.startswith("report: VMGENID_EVENT_")
        ]
        == expected,
        "Host event record order differs",
    )
    require(
        [line for line in guest if line.startswith("VMGENID_EVENT_")] == expected,
        "Guest event record order differs",
    )
    pass_positions = {
        iteration: next(
            i
            for i, line in enumerate(guest)
            if line.startswith(f"PASS {tag}_NET iteration={iteration} ")
        )
        for iteration in [1, 2, 3]
    }
    guest_order = [
        guest.index(armed[1]["marker"]),
        pass_positions[1],
        guest.index(complete[2]["marker"]),
        guest.index(armed[2]["marker"]),
        pass_positions[2],
        guest.index(complete[3]["marker"]),
        pass_positions[3],
    ]
    require(
        guest_order == sorted(guest_order) and len(set(guest_order)) == 7,
        "Guest report order differs",
    )
    stages = (root / "stages.log").read_text(encoding="utf-8").splitlines()
    watermarks = {}
    for phase in ["cold", "full", "diff"]:
        matches = [
            re.fullmatch(rf"\S+ phase={phase} watermark=([0-9]+)", line)
            for line in stages
        ]
        matches = [match for match in matches if match]
        require(len(matches) == 1, f"Expected one {phase} watermark in stages.log")
        watermarks[phase] = int(matches[0][1])
    require(
        watermarks["cold"]
        <= armed[1]["index"]
        < passes[1]["index"]
        < watermarks["full"],
        "Cold ARMED/PASS records cross the full restore watermark",
    )
    require(
        watermarks["full"]
        <= complete[2]["index"]
        < armed[2]["index"]
        < passes[2]["index"]
        < watermarks["diff"],
        "Full restore event/PASS records cross a phase watermark",
    )
    require(
        watermarks["diff"] <= complete[3]["index"] < passes[3]["index"],
        "Differential restore completion precedes its watermark",
    )
    for phase, iteration, count in [("cold", 1, 2), ("full", 2, 2), ("diff", 3, 1)]:
        require(
            sum(
                re.fullmatch(rf"\S+ phase={phase} completed={iteration}", line)
                is not None
                for line in stages
            )
            == count,
            f"Unexpected {phase} completion count in stages.log",
        )
    return {
        "scope": "Two prearmed same-PID child completions across maintained full/diff TAP restores; poll agreement and markers checked; ordinary egress/storage and S5 require the maintained verifier separately",
        "generation_ids": ids,
        "armed": armed,
        "complete": complete,
        "phase_watermarks": watermarks,
        "host_report_records": records,
        "guest_evidence": evidence,
        "host_echo_sha256": hashlib.sha256(host_raw).hexdigest(),
    }


def polling_records(raw, iterations):
    """Check exact inline read records before each existing XP pass."""
    lines = raw.splitlines()
    require(
        not any(b"FAIL VMGENID" in line for line in lines), "Generation reader failed"
    )
    actual = [
        (index, line) for index, line in enumerate(lines) if line.startswith(b"VMGENID")
    ]
    require(len(actual) == 4 * len(iterations), "Unexpected XP polling record count")
    ids = []
    for offset, iteration in enumerate(iterations):
        records = actual[4 * offset : 4 * offset + 4]
        pattern = (
            rb"VMGENID_READ iteration="
            + str(iteration).encode()
            + rb" read=1 value=([0-9a-f]{32})"
        )
        match = re.fullmatch(pattern, records[0][1])
        require(match is not None, "Invalid first XP generation read")
        value = match[1]
        require(value != b"0" * 32, "Zero XP generation ID")
        expected = [
            b"VMGENID_READ iteration=%d read=1 value=" % iteration + value,
            b"VMGENID_READ iteration=%d read=2 value=" % iteration + value,
            b"VMGENID_STAGE iteration=%d value=" % iteration + value,
            b"VMGENID=" + value,
        ]
        require(
            [line for _, line in records] == expected,
            "XP running reads or stage ID differ",
        )
        ids.append(
            {
                "iteration": iteration,
                "id": value.decode("ascii"),
                "first_index": records[0][0],
                "last_index": records[-1][0],
            }
        )
    return ids, lines


def verify_xp(root, mode, reader, expected):
    """Verify polling only; ordinary egress/storage/accounting stays separate."""
    identity = check_reader(reader, expected)
    files = {}

    def read(name, limit=16 * 1024 * 1024):
        raw = read_regular(root / name, limit)
        files[name] = {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
        return raw

    require(
        json.loads(read("generation-reader-before.json", 1024)) == identity,
        "Prelaunch and collected generation reader differ",
    )
    stages = read("stages.log").decode("ascii").splitlines()
    phases = ["cold", "full", "diff"]
    watermarks = {}
    for phase, iteration, count in [("cold", 1, 2), ("full", 2, 2), ("diff", 3, 1)]:
        marks = [
            re.fullmatch(rf"\S+ phase={phase} watermark=([0-9]+)", line)
            for line in stages
        ]
        marks = [match for match in marks if match]
        require(len(marks) == 1, f"Expected one {phase} watermark")
        watermarks[phase] = int(marks[0][1])
        require(
            sum(
                re.fullmatch(rf"\S+ phase={phase} completed={iteration}", line)
                is not None
                for line in stages
            )
            == count,
            f"Unexpected {phase} completion count",
        )
    require(
        watermarks["cold"] < watermarks["full"] < watermarks["diff"],
        "XP stage watermarks are not increasing",
    )
    ids = []
    if mode == "tap":
        for iteration, phase in enumerate(phases, 1):
            values, lines = polling_records(read(phase + ".log"), [iteration])
            passes = [
                index
                for index, line in enumerate(lines)
                if re.fullmatch(rb"XP_CHECK iteration=%d ?" % iteration, line)
            ]
            require(
                len(passes) == 1 and values[0]["last_index"] < passes[0],
                "XP polling must precede its TAP pass",
            )
            ids += values
        final, _ = polling_records(read("final-guest.txt"), [3])
        require(final[0]["id"] == ids[-1]["id"], "Final guest TAP polling differs")
        host = read("echo.log").splitlines()
        completed = [
            index
            for index, line in enumerate(host)
            if line.startswith(b"echo:") and line.endswith(b" bytes=2097152")
        ]
    else:
        require(mode == "connect", "Invalid XP polling mode")
        ids, lines = polling_records(read("guest-transcript.txt"), [1, 2, 3])
        for value in ids:
            iteration = value["iteration"]
            passes = [
                index
                for index, line in enumerate(lines)
                if line == b"XPLEGACY_PASS iteration=%d" % iteration
            ]
            require(
                len(passes) == 1 and value["last_index"] < passes[0],
                "XP polling must precede its CONNECT pass",
            )
            if iteration < 3:
                require(
                    passes[0] < ids[iteration]["first_index"],
                    "XP generation stage order differs",
                )
        host = read("proxy.log").splitlines()
        completed = [
            index
            for index, line in enumerate(host)
            if b"egress CONNECT rejected" in line
        ]
    require(len(completed) == 3, "Expected three host XP egress completions")
    for index, phase in enumerate(phases):
        require(
            watermarks[phase] <= completed[index],
            f"XP {phase} completion precedes watermark",
        )
        if index < 2:
            require(
                completed[index] < watermarks[phases[index + 1]],
                f"XP {phase} completion crosses next watermark",
            )
    values = [item["id"] for item in ids]
    require(len(set(values)) == 3, "XP cold/full/diff IDs must differ")
    return {
        "scope": "XP inline polling only; ordinary egress/storage/S5 require the maintained verifier separately",
        "mode": mode,
        "generation_ids": values,
        "reads": 6,
        "reader": identity,
        "raw_evidence": files,
        "phase_watermarks": watermarks,
        "notifications_qualified": False,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--tag", help="WINDOWS_GUEST_TAG used by the TAP run")
    parser.add_argument(
        "--collect-from",
        type=Path,
        help="Closed evidence directory on the read-only guest mount",
    )
    parser.add_argument("--xp-mode", choices=["tap", "connect"])
    parser.add_argument("--reader", type=Path)
    parser.add_argument("--probe-sha256")
    parser.add_argument(
        "--check-reader",
        help="Hash this positional reader before launching any guest helpers",
    )
    args = parser.parse_args()
    try:
        if args.check_reader is not None:
            require(
                args.tag is None
                and args.xp_mode is None
                and args.collect_from is None
                and args.reader is None
                and args.probe_sha256 is None,
                "Reader preflight cannot verify a run",
            )
            result = check_reader(args.run, args.check_reader)
        elif args.xp_mode is not None:
            require(
                args.tag == ("XP" if args.xp_mode == "tap" else "XPLEGACY")
                and args.reader is not None
                and args.probe_sha256 is not None
                and args.collect_from is None,
                "XP polling needs its actual reader/tag/SHA256",
            )
            result = verify_xp(args.run, args.xp_mode, args.reader, args.probe_sha256)
        else:
            require(
                args.tag is not None and re.fullmatch(r"[A-Za-z0-9_-]+", args.tag),
                "Invalid guest tag",
            )
            require(
                args.reader is None and args.probe_sha256 is None,
                "Reader options require XP polling mode",
            )
            if args.collect_from is not None:
                collect(args.collect_from, args.run)
            result = verify(args.run, args.tag)
        print(json.dumps(result, indent=2))
    except (OSError, UnicodeError, ValueError) as error:
        parser.exit(1, f"Generation verification failed: {error}\n")
