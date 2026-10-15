# Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
# SPDX-License-Identifier: Apache-2.0
"""Tests checking against the existence of licenses in each file."""

import datetime
import json
import sys
from pathlib import Path

import pytest

from framework import utils_repo
from framework.defs import FC_WORKSPACE_DIR
from host_tools.cargo_build import cargo

AMAZON_COPYRIGHT_YEARS = range(2018, datetime.datetime.now().year + 1)
AMAZON_COPYRIGHT = (
    "Copyright {} Amazon.com, Inc. or its affiliates. All Rights Reserved."
)
AMAZON_LICENSE = "SPDX-License-Identifier: Apache-2.0"
CHROMIUM_COPYRIGHT = "Copyright 2017 The Chromium OS Authors. All rights reserved."
CHROMIUM_LICENSE = (
    "Use of this source code is governed by a BSD-style license that can be"
)
TUNTAP_COPYRIGHT = (
    "Copyright TUNTAP, 2017 The Chromium OS Authors. All rights reserved."
)
TUNTAP_LICENSE = (
    "Use of this source code is governed by a BSD-style license that can be"
)
ALIBABA_COPYRIGHT = "Copyright (C) 2019 Alibaba Cloud Computing. All rights reserved."
ALIBABA_LICENSE = "SPDX-License-Identifier: Apache-2.0 OR BSD-3-Clause"
INTEL_COPYRIGHT = "Copyright © 2019 Intel Corporation"
INTEL_LICENSE = "SPDX-License-Identifier: Apache-2.0"
RIVOS_COPYRIGHT = "Copyright © 2023 Rivos, Inc."
RIVOS_LICENSE = "SPDX-License-Identifier: Apache-2.0"
ORACLE_COPYRIGHT = "Copyright © 2020, Oracle and/or its affiliates."
ORACLE_LICENSE = "SPDX-License-Identifier: Apache-2.0"
BLOUDRAAD_COPYRIGHT = ("Copyright 2026 Bloudraad.", "Copyright 2026 Bloudraad Ltd")
HELPER_PATH = Path("tools/uefi-vars-helper")
FIRMWARE_PROBE_PATHS = (
    Path("resources/firmware/cloudhv-console/tests"),
    Path("resources/firmware/cloudhv-secureboot/tests"),
)
FIRMWARE_LICENSE = "SPDX-License-Identifier: BSD-2-Clause-Patent"

EXCLUDE = ["build", ".kernel", ".git"]


def _has_amazon_copyright(string):
    for year in AMAZON_COPYRIGHT_YEARS:
        if AMAZON_COPYRIGHT.format(year) in string:
            return True
    return False


def _look_for_license(file, license_msg):
    line = file.readline()
    while line.startswith("//") or line.startswith("#"):
        if license_msg in line:
            return True
        line = file.readline()
    return False


def _validate_license(filename):
    """
    Validate license all .rs/.py. or .sh file.

    Python and Rust files should have the licenses on the first 2 lines
    Shell files license is located on lines 3-4 to account for shebang
    """
    filename = Path(filename)
    try:
        relative = filename.resolve().relative_to(FC_WORKSPACE_DIR)
    except ValueError:
        relative = Path()
    license_msg = AMAZON_LICENSE
    if relative.suffix == ".c" and any(
        relative.is_relative_to(path) for path in FIRMWARE_PROBE_PATHS
    ):
        license_msg = FIRMWARE_LICENSE
    with filename.open("r", encoding="utf-8") as file:
        # Find the copyright line
        while True:
            line = file.readline()
            if line.startswith(("// Copyright", "# Copyright")):
                copyright_info = line
                break
            if line == "":
                return False

        has_amazon_copyright = _has_amazon_copyright(
            copyright_info
        ) and _look_for_license(file, AMAZON_LICENSE)

        has_chromium_copyright = (
            CHROMIUM_COPYRIGHT in copyright_info
            and _look_for_license(file, CHROMIUM_LICENSE)
        )

        has_tuntap_copyright = TUNTAP_COPYRIGHT in copyright_info and _look_for_license(
            file, CHROMIUM_LICENSE
        )

        has_alibaba_copyright = (
            ALIBABA_COPYRIGHT in copyright_info
            and _look_for_license(file, ALIBABA_LICENSE)
        )

        has_intel_copyright = INTEL_COPYRIGHT in copyright_info and _look_for_license(
            file, INTEL_LICENSE
        )

        has_rivos_copyright = RIVOS_COPYRIGHT in copyright_info and _look_for_license(
            file, RIVOS_LICENSE
        )

        has_oracle_copyright = ORACLE_COPYRIGHT in copyright_info and _look_for_license(
            file, ORACLE_LICENSE
        )

        has_bloudraad_copyright = any(
            credit in copyright_info for credit in BLOUDRAAD_COPYRIGHT
        ) and _look_for_license(file, license_msg)

        return (
            has_amazon_copyright
            or has_chromium_copyright
            or has_tuntap_copyright
            or has_alibaba_copyright
            or has_intel_copyright
            or has_rivos_copyright
            or has_oracle_copyright
            or has_bloudraad_copyright
        )


def test_for_valid_licenses():
    """
    Test that all *.py, *.rs and *.sh files contain a valid license.
    """

    python_files = list(utils_repo.git_repo_files(root="..", glob="*.py"))
    rust_files = list(utils_repo.git_repo_files(root="..", glob="*.rs"))
    bash_files = list(utils_repo.git_repo_files(root="..", glob="*.sh"))
    c_files = list(utils_repo.git_repo_files(root="..", glob="*.c"))
    all_files = rust_files + python_files + bash_files + c_files
    error_msg = []
    for file in all_files:
        if _validate_license(file) is False:
            error_msg.append(file)
    assert not error_msg, f"Files {error_msg} have invalid licenses"


@pytest.mark.parametrize(
    "relative,license_id,expected",
    [
        ("src/vmm/probe.rs", "Apache-2.0", True),
        ("tools/windows/probe.rs", "Apache-2.0", True),
        ("resources/firmware/cloudhv-console/tests/probe.sh", "Apache-2.0", True),
        ("resources/firmware/cloudhv-secureboot/tests/probe.rs", "Apache-2.0", True),
        ("tools/uefi-vars-helper/src/probe.rs", "GPL-2.0-or-later", False),
        (
            "resources/firmware/cloudhv-console/tests/probe.c",
            "BSD-2-Clause-Patent",
            True,
        ),
        (
            "resources/firmware/cloudhv-secureboot/tests/probe.c",
            "BSD-2-Clause-Patent",
            True,
        ),
        ("src/vmm/probe.rs", "GPL-2.0-or-later", False),
        ("tools/windows/probe.rs", "GPL-2.0-or-later", False),
        ("tools/uefi-vars-helper-extra/probe.rs", "GPL-2.0-or-later", False),
        ("tools/uefi-vars-helper/src/probe.rs", "Apache-2.0", True),
        ("src/vmm/probe.c", "BSD-2-Clause-Patent", False),
        (
            "resources/firmware/cloudhv-console/tests-extra/probe.c",
            "BSD-2-Clause-Patent",
            False,
        ),
        ("resources/firmware/cloudhv-console/tests/probe.c", "Apache-2.0", False),
        ("src/vmm/probe.rs", "MIT", False),
    ],
)
def test_bloudraad_license_scopes(
    tmp_path, monkeypatch, relative, license_id, expected
):
    """First-party code uses Apache; firmware probes retain their BSD licence."""
    monkeypatch.setattr(sys.modules[__name__], "FC_WORKSPACE_DIR", tmp_path)
    path = tmp_path / relative
    path.parent.mkdir(parents=True)
    path.write_text(
        f"// {BLOUDRAAD_COPYRIGHT[0]}\n// SPDX-License-Identifier: {license_id}\n",
        encoding="utf-8",
    )
    assert _validate_license(path) is expected


def test_local_license_requires_copyright(tmp_path, monkeypatch):
    """An SPDX identifier alone does not satisfy the copyright requirement."""
    monkeypatch.setattr(sys.modules[__name__], "FC_WORKSPACE_DIR", tmp_path)
    path = tmp_path / HELPER_PATH / "probe.rs"
    path.parent.mkdir(parents=True)
    path.write_text(f"// {AMAZON_LICENSE}\n", encoding="utf-8")
    assert _validate_license(path) is False


def test_dependency_licenses():
    """Ensure license compatibility for Firecracker.

    For a list of currently allowed licenses checkout deny.toml in
    the root directory.
    """
    toml_file = FC_WORKSPACE_DIR / "Cargo.toml"
    _, _, stderr = cargo(
        "deny",
        f"--format json --manifest-path {toml_file} check "
        f"--config {FC_WORKSPACE_DIR / 'deny.toml'} licenses bans",
    )
    reports = [json.loads(line) for line in stderr.splitlines()]
    summary = next(
        report["fields"]["licenses"]
        for report in reports
        if report["type"] == "summary"
    )
    assert summary["errors"] == 0
    standalone = toml_file.resolve() != FC_WORKSPACE_DIR / "Cargo.toml"
    diagnostics = [
        report["fields"]
        for report in reports
        if report["type"] == "diagnostic"
        and "license" in report["fields"]["code"]
        and not (standalone and report["fields"]["code"] == "license-not-encountered")
    ]
    assert not diagnostics
