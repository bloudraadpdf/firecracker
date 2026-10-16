#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
checker=$(cd "$(dirname "$0")" && pwd)/verify-secure-state.sh
fixture=${1:?Set a new absolute workspace fixture directory}
[[ "$fixture" = /* ]]
mkdir "$fixture"
trap 'rm -r "$fixture"' EXIT
printf 'TPM_STATE PCR7_SHA256=%064d\nTPM_STATE HANDLE=81000001\nTPM_STATE PUBLIC=001122\nTPM_STATE NAME=000b1122\n' 0 > "$fixture/baseline"
for name in PK KEK db dbx; do printf 'UEFI_STATE %s=%064d\n' "$name" 0; done > "$fixture/uefi-baseline"
for scenario in exact wrapped missing changed extra uefi-missing uefi-changed uefi-extra uefi-name \
    runtime-missing runtime-absent runtime-label runtime-order runtime-duplicate runtime-extra runtime-zero \
    runtime-negative runtime-hex runtime-max runtime-overflow runtime-error-missing runtime-fail \
    runtime-phase-missing runtime-phase-changed runtime-phase-duplicate \
    runtime-phase-stale runtime-phase-order runtime-cold runtime-trailing; do
    copies=3
    if [[ "$scenario" = missing ]]; then copies=2; fi
    if [[ "$scenario" = extra ]]; then copies=4; fi
    for ((index=0; index<copies; index++)); do cat "$fixture/baseline"; done > "$fixture/raw"
    case "$scenario" in
        wrapped) fold -w 40 "$fixture/raw" > "$fixture/lines" ;;
        changed) sed '$s/000b1122/000b1123/' "$fixture/raw" > "$fixture/lines" ;;
        *) cp "$fixture/raw" "$fixture/lines" ;;
    esac
    copies=3
    if [[ "$scenario" = uefi-missing ]]; then copies=2; fi
    if [[ "$scenario" = uefi-extra ]]; then copies=4; fi
    for ((index=0; index<copies; index++)); do cat "$fixture/uefi-baseline"; done > "$fixture/uefi-raw"
    case "$scenario" in
        wrapped) fold -w 40 "$fixture/uefi-raw" >> "$fixture/lines" ;;
        uefi-changed) sed '$s/0$/1/' "$fixture/uefi-raw" >> "$fixture/lines" ;;
        uefi-name) sed '$s/dbx=/db=/' "$fixture/uefi-raw" >> "$fixture/lines" ;;
        *) cat "$fixture/uefi-raw" >> "$fixture/lines" ;;
    esac
    for phase in 1 2 3; do
        for label in SetupMode SecureBoot tampered wrong-owner replay; do
            printf 'UEFI_RUNTIME rejected=%s win32_error=%s\n' "$label" "$phase"
        done
        printf 'PASS UEFI_RUNTIME phase=%s\n' "$phase"
    done > "$fixture/runtime"
    case "$scenario" in
        runtime-absent) : ;;
        runtime-missing) sed '3d' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-label) sed 's/rejected=wrong-owner/rejected=owner/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-order) sed '1s/SetupMode/SecureBoot/;2s/SecureBoot/SetupMode/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-duplicate) sed '3p' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-extra) cat "$fixture/runtime" "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-zero) sed '3s/1$/0/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-negative) sed '3s/1$/-1/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-max) sed 's/win32_error=[123]$/win32_error=4294967295/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-overflow) sed '3s/1$/4294967296/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-hex) sed '3s/1$/0x522/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-error-missing) sed '3s/ win32_error=1$//' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-fail) cat "$fixture/runtime" >> "$fixture/lines"; echo 'FAIL UEFI_RUNTIME fixture' >> "$fixture/lines" ;;
        runtime-phase-missing) sed '6d' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-phase-changed) sed '6s/phase=1/phase=4/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-phase-duplicate) sed '6p' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-phase-stale) sed '12s/phase=2/phase=1/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-phase-order) sed '6s/phase=1/phase=2/;12s/phase=2/phase=1/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-cold) sed '18s/phase=3/phase=cold/' "$fixture/runtime" >> "$fixture/lines" ;;
        runtime-trailing) cat "$fixture/runtime" >> "$fixture/lines"; echo 'UEFI_RUNTIME rejected=replay win32_error=1314' >> "$fixture/lines" ;;
        *) cat "$fixture/runtime" >> "$fixture/lines" ;;
    esac
    awk '{printf "%s\r\n", $0}' "$fixture/lines" > "$fixture/transcript"
    result=0
    bash "$checker" "$fixture/transcript" "$fixture/baseline" "$fixture/uefi-baseline" > "$fixture/result.log" 2>&1 || result=$?
    expected=1
    if [[ "$scenario" = exact || "$scenario" = wrapped || "$scenario" = runtime-max ]]; then expected=0; fi
    if [[ "$result" != "$expected" ]]; then
        cat "$fixture/result.log"
        echo "$scenario: expected exit $expected, got $result" >&2
        exit 1
    fi
    echo "$scenario: expected exit $expected"
done
