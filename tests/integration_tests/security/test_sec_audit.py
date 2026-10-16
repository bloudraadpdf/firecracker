# Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
# SPDX-License-Identifier: Apache-2.0
"""Tests ensuring security vulnerabilities are not present in dependencies."""

import json
import os
import shlex
import sys
from pathlib import Path

import pytest

from framework import ab_test, utils, utils_repo
from framework.ab_test import (
    git_ab_test_host_command_if_pr,
    set_did_not_grow_comparator,
)


@pytest.fixture(scope="module")
def cargo_deny():
    """Install the advisory checker once for the workspace audits."""
    utils.check_output("cargo install --locked cargo-deny --debug")


@pytest.mark.usefixtures("cargo_deny")
@pytest.mark.parametrize(
    "manifest",
    [
        Path("Cargo.toml"),
        *(
            directory.relative_to("..") / "Cargo.toml"
            for directory in utils_repo.rust_tool_directories()
        ),
    ],
    ids=str,
)
def test_cargo_audit(manifest):
    """
    Run cargo audit to check for crates with security vulnerabilities.
    """

    def set_of_vulnerabilities(output: utils.CommandReturn):
        # Each stderr line contains one JSON event.
        findings = set()
        has_error_diagnostic = False
        for line in output.stderr.splitlines():
            line = line.strip()
            if not line:
                continue
            entry = json.loads(line)
            # There is also `summary` type, which is of not interest for us
            if entry["type"] != "diagnostic":
                continue
            fields = entry["fields"]
            has_error_diagnostic |= fields.get("severity") == "error"
            advisory = fields.get("advisory") or {}
            # Identify a finding by its code, advisory id and affected crate;
            # Findings without an advisory (e.g. yanked crates) fall back to
            # the crate from the dependency graph.
            krate = (fields.get("graphs") or [{}])[0].get("Krate", {})
            findings.add(
                (
                    fields.get("code"),
                    advisory.get("id"),
                    advisory.get("package") or krate.get("name"),
                )
            )
        assert output.returncode == 0 or (
            output.returncode == 1 and has_error_diagnostic
        ), f"cargo deny failed without a valid advisory result:\n{output}"
        return findings

    assert (Path.cwd().parent / manifest).is_file()
    toml_file = shlex.quote(str(manifest))
    command = f"RUSTUP_LOG=warn cargo deny --locked --manifest-path {toml_file} -f json check advisories"
    if manifest != Path("Cargo.toml"):
        command = f"if [ -f {toml_file} ]; then {command}; fi"

    git_ab_test_host_command_if_pr(
        command,
        comparator=set_did_not_grow_comparator(set_of_vulnerabilities),
    )


@pytest.fixture
def audit_checkouts(tmp_path, monkeypatch):
    """Record actual audit commands in isolated A/B checkout fixtures."""
    baseline = tmp_path / "baseline checkout"
    candidate = tmp_path / "candidate checkout"
    for checkout in [baseline, candidate]:
        (checkout / "tests").mkdir(parents=True)
        (checkout / "Cargo.toml").touch()
    capture = tmp_path / "commands.jsonl"
    cargo = tmp_path / "cargo"
    cargo.write_text(
        f"#!{sys.executable}\n"
        "import json, os, pathlib, sys\n"
        "if sys.argv[1] != 'install':\n"
        "    manifest = pathlib.Path(sys.argv[sys.argv.index('--manifest-path') + 1]).resolve()\n"
        "    with open(os.environ['AUDIT_CHECKOUT_LOG'], 'a') as log:\n"
        "        log.write(json.dumps(str(manifest)) + '\\n')\n"
        "    if os.environ.get('AUDIT_FIXTURE_ADVISORY') == '1':\n"
        "        print(json.dumps({'type': 'diagnostic', 'fields': {'code': 'vulnerability',\n"
        "            'severity': os.environ.get('AUDIT_FIXTURE_SEVERITY', 'error'),\n"
        "            'advisory': {'id': 'RUSTSEC-2023-0071', 'package': 'rsa'}}}), file=sys.stderr)\n"
        "    if pathlib.Path.cwd().name == 'candidate checkout':\n"
        "        status = os.environ.get('AUDIT_FIXTURE_EXIT')\n"
        "        if status is not None:\n"
        "            sys.exit(int(status))\n"
        "    if os.environ.get('AUDIT_FIXTURE_ADVISORY') == '1':\n"
        "        sys.exit(1)\n"
    )
    cargo.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}:{os.environ['PATH']}")
    monkeypatch.setenv("AUDIT_CHECKOUT_LOG", str(capture))
    monkeypatch.setattr(ab_test.global_props, "buildkite_pr", True)
    monkeypatch.chdir(candidate / "tests")

    def compare_checkouts(test_runner, comparator, **_kwargs):
        output_a = test_runner(baseline, True)
        output_b = test_runner(candidate, False)
        return output_a, output_b, comparator(output_a, output_b)

    monkeypatch.setattr(ab_test, "git_ab_test", compare_checkouts)
    return baseline, candidate, capture


@pytest.mark.parametrize(
    "manifest", [Path("Cargo.toml"), Path("tools/existing tool/Cargo.toml")], ids=str
)
def test_cargo_audit_uses_checkout_manifest(audit_checkouts, manifest):
    """Audit each revision's manifest rather than the original checkout."""
    baseline, candidate, capture = audit_checkouts
    for checkout in [baseline, candidate]:
        (checkout / manifest).parent.mkdir(parents=True, exist_ok=True)
        (checkout / manifest).touch()
    test_cargo_audit(manifest)
    assert [Path(json.loads(line)) for line in capture.read_text().splitlines()] == [
        baseline / manifest,
        candidate / manifest,
    ]


def test_cargo_audit_missing_baseline_tool(audit_checkouts):
    """A new tool skips only its missing baseline manifest, not the root audit."""
    baseline, candidate, capture = audit_checkouts
    manifest = Path("tools/new tool/Cargo.toml")
    (candidate / manifest).parent.mkdir(parents=True)
    (candidate / manifest).touch()
    test_cargo_audit(Path("Cargo.toml"))
    test_cargo_audit(manifest)
    assert [Path(json.loads(line)) for line in capture.read_text().splitlines()] == [
        baseline / "Cargo.toml",
        candidate / "Cargo.toml",
        candidate / manifest,
    ]


def test_cargo_audit_new_tool_finding_fails(audit_checkouts, monkeypatch):
    """Reject an advisory introduced by a new standalone workspace."""
    _, candidate, capture = audit_checkouts
    manifest = Path("tools/new-tool/Cargo.toml")
    (candidate / manifest).parent.mkdir(parents=True)
    (candidate / manifest).touch()
    monkeypatch.setenv("AUDIT_FIXTURE_ADVISORY", "1")
    with pytest.raises(AssertionError, match="changed"):
        test_cargo_audit(manifest)
    assert [Path(json.loads(line)) for line in capture.read_text().splitlines()] == [
        candidate / manifest
    ]


def test_cargo_audit_finding_fails_outside_pr(audit_checkouts, monkeypatch):
    """A standalone run must propagate cargo-deny's nonzero status."""
    _, candidate, capture = audit_checkouts
    monkeypatch.setattr(ab_test.global_props, "buildkite_pr", False)
    monkeypatch.setenv("AUDIT_FIXTURE_ADVISORY", "1")
    with pytest.raises(ChildProcessError, match="RUSTSEC-2023-0071"):
        test_cargo_audit(Path("Cargo.toml"))
    assert [Path(json.loads(line)) for line in capture.read_text().splitlines()] == [
        candidate / "Cargo.toml"
    ]


@pytest.mark.parametrize(
    "status,severity", [(1, None), (137, None), (1, "warning"), (137, "error")]
)
def test_cargo_audit_rejects_incomplete_failure(
    audit_checkouts, monkeypatch, status, severity
):
    """Do not accept a failed or interrupted audit as an empty finding set."""
    _, _, capture = audit_checkouts
    monkeypatch.setenv("AUDIT_FIXTURE_EXIT", str(status))
    if severity is not None:
        monkeypatch.setenv("AUDIT_FIXTURE_ADVISORY", "1")
        monkeypatch.setenv("AUDIT_FIXTURE_SEVERITY", severity)
    with pytest.raises(AssertionError, match="without a valid advisory result"):
        test_cargo_audit(Path("Cargo.toml"))
    assert len(capture.read_text().splitlines()) == 2


def test_cargo_audit_retains_existing_findings(audit_checkouts, monkeypatch):
    """An unchanged advisory remains eligible for the existing A/B comparison."""
    _, _, capture = audit_checkouts
    monkeypatch.setenv("AUDIT_FIXTURE_ADVISORY", "1")
    test_cargo_audit(Path("Cargo.toml"))
    assert len(capture.read_text().splitlines()) == 2
