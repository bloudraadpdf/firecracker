#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
base=${WINDOWS_CHECK_BASE:?Set test directory}
run=$base/${WINDOWS_CHECK_RUN:-run1}
tag=${WINDOWS_GUEST_TAG:-WIN7}
guest=${WINDOWS_GUEST_ADDRESS:-172.30.254.2}
offset=${WINDOWS_PARTITION_OFFSET:?Set partition byte offset}
disk=${WINDOWS_CHECK_DISK:-$base/windows.raw}
encoding=${WINDOWS_TRANSCRIPT_ENCODING:-UTF-8}
mkdir -p "$base/mnt"
if mountpoint -q "$base/mnt"; then exit 1; fi
bash "$(dirname "$0")/assert-disk-closed.sh" "$disk"
if [[ "$tag" = XP ]]; then
    iteration=1
    for phase in cold full diff; do
        grep -q "XP_CHECK iteration=$iteration" "$run/$phase.log"
        if grep -q XP_CHECK_FAILED "$run/$phase.log"; then exit 1; fi
        iteration=$((iteration + 1))
    done
else
    for iteration in 1 2 3; do
        test "$(grep -c "^report: PASS ${tag}_NET iteration=$iteration " "$run/echo.log")" = 1
    done
fi
test "$(grep -F "echo: peer=$guest:" "$run/echo.log" | grep -c ' bytes=2097152$')" = 3
test "$(grep -c '^echo:' "$run/echo.log")" = 3
if grep -q "FAIL ${tag}_NET" "$run/echo.log"; then exit 1; fi
grep -q 'Guest requested ACPI S5 shutdown' "$run/diff.log"
grep -q 'Firecracker exiting successfully. exit_code=0' "$run/diff.log"
loop=$(losetup --read-only --find --show --offset "$offset" "$disk")
trap 'if mountpoint -q "$base/mnt"; then umount "$base/mnt"; fi; losetup -d "$loop"' EXIT
ntfs-3g -o ro "$loop" "$base/mnt"
test ! -e "$base/mnt/hiberfil.sys"
if [[ "$tag" = XP ]]; then
    cp "$base/mnt/fc-restore-data.bin" "$run/final-data.bin"
    tr -d '\r' < "$base/mnt/fc-restore-probe.txt" > "$run/final-guest.txt"
    seed=$(sed -n 's/^PASS XP_STORAGE bytes=65536 seed=//p' "$run/final-guest.txt")
    "${WINDOWS_XP_VERIFIER:?Set the compiled XP pattern verifier}" "$run/final-data.bin" "$seed"
    grep -q '^HTTP status=200$' "$run/final-guest.txt"
    grep -q '^PASS XP_TCP_EQUAL bytes=2097152$' "$run/final-guest.txt"
    sha256sum "$run/final-data.bin"
else
    cp "$base/mnt/Users/Public/fc-restore-data.bin" "$run/final-data.bin"
    cat "$base/mnt/Users/Public/fc-network.log" | iconv -f "$encoding" -t UTF-8 > "$run/final-guest.txt"

    for iteration in 1 2 3; do
        test "$(grep -c "^PASS ${tag}_NET iteration=$iteration " "$run/final-guest.txt")" = 1
        if [[ ${WINDOWS_SECURE_CHECKS:-0} = 1 ]]; then
            test "$(grep -c "^PASS SECURE_STATE iteration=$iteration SecureBoot GetTpm TBS_GetRandom" "$run/final-guest.txt")" = 1
        fi
    done
    expected=$(sed -n 's/.*iteration=3.*STORAGE_SHA256=\([0-9a-f]*\).*/\1/p' "$run/echo.log")
    test "${#expected}" = 64
    printf '%s  %s\n' "$expected" "$run/final-data.bin" | sha256sum -c -
    test "$(stat -c %s "$run/final-data.bin")" = 65536
    grep -E "PASS ${tag}_|HTTPS|HTTP status" "$run/final-guest.txt"
fi
