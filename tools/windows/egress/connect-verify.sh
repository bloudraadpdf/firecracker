#!/bin/bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
checker=$(realpath "$(dirname "$0")/assert-disk-closed.sh")
secure_checker=$(realpath "$(dirname "$0")/verify-secure-state.sh")
cd "${WINDOWS_CONNECT_BASE:?Set test directory}"
prefix=${WINDOWS_CONNECT_PREFIX:?Set guest file prefix}
tag=${WINDOWS_CONNECT_TAG:?Set guest marker tag}
identity=${WINDOWS_CONNECT_IDENTITY:-SYSTEM}
identity_label=${WINDOWS_CONNECT_IDENTITY_LABEL:-RunAs User}
encoding=${WINDOWS_CONNECT_ENCODING:-UTF-8}
offset=${WINDOWS_CONNECT_OFFSET:-}
disk=$(realpath "${WINDOWS_CONNECT_DISK:-windows.raw}")
bash "$checker" "$disk"
grep -q 'Guest requested ACPI S5 shutdown' run/diff.log
grep -q 'Firecracker exiting successfully. exit_code=0' run/diff.log
accounting=$(grep '^ACCOUNTING ' run/proxy.log)
test "$(printf '%s' "$accounting" | grep -o 'FetchLogEntry' | wc -l)" = 6
allowed='dest_host: "127.0.0.1", dest_port: 40567, bytes_up: 2097152, bytes_down: 2097152, outcome: Connected'
denied='dest_host: "127.0.0.1", dest_port: 40568, bytes_up: 0, bytes_down: 0, outcome: PolicyDenied'
test "$(printf '%s' "$accounting" | grep -oF "$allowed" | wc -l)" = 3
test "$(printf '%s' "$accounting" | grep -oF "$denied" | wc -l)" = 3
mkdir -p mnt
if mountpoint -q mnt; then exit 1; fi
loop_args=(--find --show --read-only)
if [[ -n "$offset" ]]; then loop_args+=(--offset "$offset"); else loop_args+=(--partscan); fi
loop=$(losetup "${loop_args[@]}" "$disk")
cleanup() {
    if mountpoint -q mnt; then umount mnt; fi
    losetup -d "$loop"
}
trap cleanup EXIT
partition=$loop
if [[ -z "$offset" ]]; then partition=${loop}p3; fi
ntfs-3g "$partition" mnt -o ro
test ! -e mnt/hiberfil.sys
iconv -f "$encoding" -t UTF-8 "mnt/$prefix-probe.log" > run/guest-transcript.txt
cp "mnt/$prefix-data.bin" run/final-data.bin
if [[ ${WINDOWS_SECURE_CHECKS:-0} = 1 ]]; then
    cp mnt/Users/Public/fc-tpm-state.txt run/tpm-state.txt
    bash "$secure_checker" run/guest-transcript.txt run/tpm-state.txt
fi
for iteration in 1 2 3; do
    test "$(grep -c "^${tag}_PASS iteration=$iteration" run/guest-transcript.txt)" = 1
    if [[ ${WINDOWS_SECURE_CHECKS:-0} = 1 ]]; then
        test "$(grep -c "^PASS SECURE_STATE iteration=$iteration SecureBoot GetTpm TBS_GetRandom" run/guest-transcript.txt)" = 1
    fi
done
if grep -q "${tag}_FAIL" run/guest-transcript.txt; then exit 1; fi
if [[ "$tag" = XPLEGACY ]]; then
    test "$(grep -c '^CONNECT_RESULT=0' run/guest-transcript.txt)" = 1
    "${WINDOWS_CONNECT_XP_VERIFIER:?Set the legacy XP host verifier}" run/final-data.bin
else
    grep -Ei "${identity_label}[[:space:]]*:.*\\\\${identity}[[:space:]]*$" run/guest-transcript.txt
    expected=$(awk '{gsub(/\r/, "")} $1 == "Hash" && $2 == ":" {hash=$3; reading=1; next} reading && $1 ~ /^[0-9A-Fa-f]+$/ {hash=hash $1; next} reading {reading=0} END {print hash}' run/guest-transcript.txt | tr -d '\r')
    test "${#expected}" = 64
    printf '%s  %s\n' "$expected" run/final-data.bin | sha256sum -c -
fi
test "$(stat -c %s run/final-data.bin)" = 2097152
grep "^${tag}_PASS" run/guest-transcript.txt
echo 'Verified three guest passes, six sealed accounting entries and final disk data'
