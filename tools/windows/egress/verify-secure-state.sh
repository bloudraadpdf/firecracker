#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
transcript=${1:?Set guest transcript}
baseline=$(tr -d '\r' < "${2:?Set guest TPM baseline}")
test "$(printf '%s\n' "$baseline" | grep -c '^')" = 4
printf '%s\n' "$baseline" | sed -n '1p' | grep -Eq '^TPM_STATE PCR7_SHA256=[0-9a-f]{64}$'
printf '%s\n' "$baseline" | sed -n '2p' | grep -Eq '^TPM_STATE HANDLE=81[0-9a-f]{6}$'
printf '%s\n' "$baseline" | sed -n '3p' | grep -Eq '^TPM_STATE PUBLIC=([0-9a-f]{2})+$'
printf '%s\n' "$baseline" | sed -n '4p' | grep -Eq '^TPM_STATE NAME=([0-9a-f]{2})+$'
actual=$(tr -d '\r' < "$transcript" | awk '
    /^TPM_STATE / { if (line != "") print line; line=$0; next }
    line != "" && /^[0-9a-f]+$/ { line=line $0; next }
    { if (line != "") print line; line="" }
    END { if (line != "") print line }
')
expected=$(printf '%s\n%s\n%s\n' "$baseline" "$baseline" "$baseline")
test "$actual" = "$expected"
echo 'Verified unchanged TPM PCR7 and persistent object across three phases'
