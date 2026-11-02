#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
[[ $# = 5 ]]
config=$(realpath "$1")
disk=$(realpath "$2")
offset=$3
reader=$4
expected=$5
[[ "$offset" =~ ^(0|[1-9][0-9]*)$ && "$expected" =~ ^[0-9a-fA-F]{64}$ ]]
[[ "$reader" = restore-probe.exe || "$reader" = xp-connect.exe ]]
jq -e --arg disk "$disk" '[.drives[] | select(.path_on_host == $disk and .is_read_only != true)] | length == 1' "$config" >/dev/null
script_directory=$(realpath "$(dirname "$0")")
guard=${WINDOWS_DISK_GUARD:-$script_directory/../egress/assert-disk-closed.sh}
mount_helper=${WINDOWS_GENERATION_MOUNT_HELPER:-$script_directory/../egress/mount-state.sh}
verifier=${WINDOWS_GENERATION_READER_VERIFY:-$script_directory/verify.py}
bash "$guard" --config "$config"
cd "$(dirname "$config")"
directory=$(mktemp -d "$PWD/.generation-reader.XXXXXX")
interrupted=0
trap 'interrupted=130' INT
trap 'interrupted=143' TERM
set +e
(
    set -e
    # shellcheck source=../egress/mount-state.sh
    source "$mount_helper"
    windows_mount_directory=$directory
    windows_mount_attach "$disk" "$offset"
    ntfs-3g -o ro,norecover "$windows_mount_loop" "$windows_mount_directory"
    python3 "$verifier" "$windows_mount_directory/$reader" --check-reader "$expected"
)
result=$?
set -e
if ! rmdir "$directory"; then
    if [[ "$result" = 0 ]]; then result=1; fi
fi
if [[ "$interrupted" != 0 ]]; then result=$interrupted; fi
exit "$result"
