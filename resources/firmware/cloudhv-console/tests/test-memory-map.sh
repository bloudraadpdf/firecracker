#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
source_file=${1:?usage: test-memory-map.sh PATCHED_MEMDETECT_C OUTPUT_DIRECTORY}
output=${2:?usage: test-memory-map.sh PATCHED_MEMDETECT_C OUTPUT_DIRECTORY}
edk2=$(cd -- "$(dirname -- "$source_file")/../../.." && pwd)
fixture=$edk2/OvmfPkg/CloudHv/Tests/memory-map-fixtures.c
test -f "$fixture"
mkdir -p "$output"
for function in PlatformAddHobCB GetPvhMemmapEntries PlatformScanE820Pvh; do
    test "$(grep -c "^${function} (" "$source_file")" -eq 1
done
{
    printf 'static void\n'
    sed -n '/^PlatformAddHobCB (/ ,/^}/p' "$source_file"
    printf 'EFI_STATUS\n'
    sed -n '/^GetPvhMemmapEntries (/ ,/^}/p' "$source_file"
    printf 'static EFI_STATUS\n'
    sed -n '/^PlatformScanE820Pvh (/ ,/^}/p' "$source_file"
} > "$output/memory-map-functions.inc"
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -fno-pie -no-pie \
    -I "$output" "$fixture" -o "$output/memory-map-fixtures"
"$output/memory-map-fixtures"
