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
generation=${WINDOWS_GENERATION_CHECKS:-0}
if [[ "$generation" != 0 && "$generation" != 1 ]]; then
    echo 'WINDOWS_GENERATION_CHECKS must be 0 or 1' >&2
    exit 1
fi
if [[ "$generation" = 1 && "$tag" = XP ]]; then
    echo 'XP does not support prearmed generation notifications' >&2
    exit 1
fi
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
# shellcheck source=mount-state.sh
source "$(dirname "$0")/mount-state.sh"
windows_mount_directory=$base/mnt
windows_mount_loop=$(losetup --read-only --find --show --offset "$offset" "$disk")
ntfs-3g -o ro "$windows_mount_loop" "$windows_mount_directory"
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
    if [[ ${WINDOWS_SECURE_CHECKS:-0} = 1 ]]; then
        cp "$base/mnt/Users/Public/fc-tpm-state.txt" "$run/tpm-state.txt"
        cp "$base/mnt/Users/Public/fc-uefi-state.txt" "$run/uefi-state.txt"
        bash "$(dirname "$0")/verify-secure-state.sh" "$run/final-guest.txt" "$run/tpm-state.txt" "$run/uefi-state.txt"
    fi

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
    if [[ "$generation" = 1 ]]; then
        python3 "$(dirname "$0")/../generation/verify.py" "$run" --tag "$tag" \
            --collect-from "$base/mnt/Users/Public/fc-gencounter-events" \
            > "$run/generation-verification.json"
    fi
fi
