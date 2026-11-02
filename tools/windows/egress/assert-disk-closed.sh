#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
[[ $EUID = 0 ]] || { echo 'Run disk inspection as root' >&2; exit 1; }
check_disk() {
    local disk descriptor loops
    disk=$(realpath "$1")
    test -f "$disk"
    if ! loops=$(losetup --list --associated "$disk" --noheadings --output NAME); then
        echo "Cannot inspect loop devices for Windows disk: $disk" >&2
        exit 1
    fi
    if [[ -n "$loops" ]]; then
        echo "Windows disk is attached to a loop device: $loops" >&2
        exit 1
    fi
    for descriptor in /proc/[0-9]*/fd/*; do
        if [[ "$descriptor" -ef "$disk" ]]; then
            echo "Windows disk is still open: $descriptor" >&2
            exit 1
        fi
    done
}
if [[ ${1:-} = --config ]]; then
    config=${2:?Set the guest config}
    jq -e '.drives | type == "array" and length > 0 and all(.[]; .is_read_only == true or (.path_on_host | type == "string"))' "$config" >/dev/null
    while IFS= read -r -d '' path; do
        check_disk "$path"
    done < <(jq -j '.drives[] | select(.is_read_only != true) | .path_on_host + "\u0000"' "$config")
else
    disk=$(realpath "${1:?Set the guest disk}")
    check_disk "$disk"
fi
