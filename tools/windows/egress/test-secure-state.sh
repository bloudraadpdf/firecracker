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
for scenario in exact wrapped missing changed extra; do
    copies=3
    if [[ "$scenario" = missing ]]; then copies=2; fi
    if [[ "$scenario" = extra ]]; then copies=4; fi
    for ((index=0; index<copies; index++)); do cat "$fixture/baseline"; done > "$fixture/raw"
    case "$scenario" in
        wrapped) fold -w 40 "$fixture/raw" > "$fixture/lines" ;;
        changed) sed '$s/000b1122/000b1123/' "$fixture/raw" > "$fixture/lines" ;;
        *) cp "$fixture/raw" "$fixture/lines" ;;
    esac
    awk '{printf "%s\r\n", $0}' "$fixture/lines" > "$fixture/transcript"
    result=0
    bash "$checker" "$fixture/transcript" "$fixture/baseline" > "$fixture/result.log" 2>&1 || result=$?
    expected=1
    if [[ "$scenario" = exact || "$scenario" = wrapped ]]; then expected=0; fi
    if [[ "$result" != "$expected" ]]; then
        cat "$fixture/result.log"
        echo "$scenario: expected exit $expected, got $result" >&2
        exit 1
    fi
    echo "$scenario: expected exit $expected"
done
