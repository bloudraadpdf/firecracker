#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
transcript=${1:?Set guest transcript}
verify_state() {
    local baseline prefix actual expected index=1 pattern
    baseline=$(tr -d '\r' < "$1")
    prefix=$2
    shift 2
    test "$(printf '%s\n' "$baseline" | grep -c '^')" = "$#"
    for pattern in "$@"; do
        printf '%s\n' "$baseline" | sed -n "${index}p" | grep -Eq "$pattern"
        index=$((index + 1))
    done
    actual=$(tr -d '\r' < "$transcript" | awk -v prefix="$prefix" '
        index($0, prefix) == 1 { if (line != "") print line; line=$0; next }
        line != "" && /^[0-9a-f]+$/ { line=line $0; next }
        { if (line != "") print line; line="" }
        END { if (line != "") print line }
    ')
    expected=$(printf '%s\n%s\n%s\n' "$baseline" "$baseline" "$baseline")
    test "$actual" = "$expected"
}
verify_state "${2:?Set guest TPM baseline}" 'TPM_STATE ' \
    '^TPM_STATE PCR7_SHA256=[0-9a-f]{64}$' \
    '^TPM_STATE HANDLE=81[0-9a-f]{6}$' \
    '^TPM_STATE PUBLIC=([0-9a-f]{2})+$' \
    '^TPM_STATE NAME=([0-9a-f]{2})+$'
verify_state "${3:?Set guest UEFI baseline}" 'UEFI_STATE ' \
    '^UEFI_STATE PK=[0-9a-f]{64}$' \
    '^UEFI_STATE KEK=[0-9a-f]{64}$' \
    '^UEFI_STATE db=[0-9a-f]{64}$' \
    '^UEFI_STATE dbx=[0-9a-f]{64}$'
tr -d '\r' < "$transcript" | awk '
    BEGIN {
        split("SetupMode SecureBoot tampered wrong-owner replay", labels, " ")
        phase=1
        step=0
    }
    /FAIL/ { exit 1 }
    /UEFI_RUNTIME/ {
        if (phase > 3) exit 1
        if (step < 5) {
            expected="^UEFI_RUNTIME rejected=" labels[step + 1] " win32_error=[1-9][0-9]*$"
            if ($0 !~ expected) exit 1
            split($0, fields, "=")
            if (fields[3] + 0 > 4294967295) exit 1
            step++
        } else {
            if ($0 != "PASS UEFI_RUNTIME phase=" phase) exit 1
            phase++
            step=0
        }
    }
    END { if (phase != 4 || step != 0) exit 1 }
'
echo 'Verified unchanged UEFI policy, TPM PCR7 and persistent object and UEFI runtime updates across three phases'
