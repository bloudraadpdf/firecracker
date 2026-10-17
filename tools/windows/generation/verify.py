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
    contents = {}
    for name, limit in EVENT_FILES.items():
        path = directory / name
        info = path.lstat()
        require(
            path.resolve() == path and stat.S_ISREG(info.st_mode),
            f"Not a regular evidence file: {name}",
        )
        require(info.st_size <= limit, f"Evidence too large: {name}")
        with path.open("rb") as source:
            contents[name] = source.read(limit + 1)
        require(len(contents[name]) == info.st_size, f"Evidence changed: {name}")
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument(
        "--tag", required=True, help="WINDOWS_GUEST_TAG used by the TAP run"
    )
    parser.add_argument(
        "--collect-from",
        type=Path,
        help="Closed evidence directory on the read-only guest mount",
    )
    args = parser.parse_args()
    try:
        require(re.fullmatch(r"[A-Za-z0-9_-]+", args.tag), "Invalid guest tag")
        if args.collect_from is not None:
            collect(args.collect_from, args.run)
        print(json.dumps(verify(args.run, args.tag), indent=2))
    except (OSError, UnicodeError, ValueError) as error:
        parser.exit(1, f"Generation verification failed: {error}\n")
