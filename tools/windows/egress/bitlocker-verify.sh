#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
[[ $EUID = 0 ]] || { echo 'Run verification as root' >&2; exit 1; }
case ${1:-} in tap|connect) mode=$1 ;; *) echo 'usage: bitlocker-verify.sh tap|connect' >&2; exit 1 ;; esac
scripts=$(realpath "$(dirname "$0")")
disk=${WINDOWS_BITLOCKER_DISK:?Set the closed encrypted disk}
offset=${WINDOWS_BITLOCKER_OFFSET:?Set the partition byte offset}
size=${WINDOWS_BITLOCKER_SIZE:?Set the partition byte length}
state=${WINDOWS_BITLOCKER_STATE:?Set a new absolute mount directory}
bdemount=${WINDOWS_BITLOCKER_BDEMOUNT:?Set the pinned bdemount binary}
[[ "$disk" = /* && "$state" = /* && "$bdemount" = /* ]]
[[ "$offset" =~ ^[0-9]+$ && "$size" =~ ^[1-9][0-9]*$ ]]
"$bdemount" -V | grep -x 'bdemount 20260916' > /dev/null
bash "$scripts/assert-disk-closed.sh" "$disk"
umask 077
mkdir -m 700 "$state"
mkdir "$state/bde"
# shellcheck source=mount-state.sh
source "$scripts/mount-state.sh"
windows_mount_directory=$state/bde
cleanup() {
    local result=$?
    trap - EXIT
    trap '' INT TERM
    if [[ "$windows_mount_attempted" == 1 ]]; then
        windows_mount_unmount "$state/bde" fusermount -u || { if [[ "$result" = 0 ]]; then result=1; fi; }
        windows_mount_detach || { if [[ "$result" = 0 ]]; then result=1; fi; }
    fi
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
windows_mount_attach "$disk" "$offset" "$size"
"$bdemount" -u -X ro "$windows_mount_loop" "$state/bde" > "$state/mount.log" 2>&1
for ((retry = 0; retry < 50; retry++)); do
    if mountpoint -q "$state/bde"; then break; fi
    sleep 0.1
done
mountpoint -q "$state/bde"
test -f "$state/bde/bde1"
if [[ "$mode" = tap ]]; then
    WINDOWS_CHECK_DISK=$state/bde/bde1 WINDOWS_PARTITION_OFFSET=0 bash "$scripts/tap-verify.sh"
else
    WINDOWS_CONNECT_DISK=$state/bde/bde1 WINDOWS_CONNECT_OFFSET=0 bash "$scripts/connect-verify.sh"
fi
