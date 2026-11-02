#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
# Sourced before creating the offline verification mount and loop device.
windows_mount_directory=
windows_mount_loop=
windows_mount_disk=
windows_mount_identity=
windows_mount_offset=0
windows_mount_size=0
windows_mount_partscan=0
windows_mount_attempted=0

windows_mount_require_unmounted() {
    local result=0
    mountpoint -q "$1" || result=$?
    [[ "$result" == 32 ]] || {
        echo 'Windows verification directory is mounted or cannot be checked' >&2
        return 1
    }
}

windows_mount_unmount() {
    local mount_directory=$1 result=0
    shift
    mountpoint -q "$mount_directory" || result=$?
    case "$result" in
        0) "$@" "$mount_directory" || return
           windows_mount_require_unmounted "$mount_directory" ;;
        32) return 0 ;;
        *) echo 'Cannot check Windows verification mount for cleanup' >&2; return 1 ;;
    esac
}

windows_mount_observe() {
    local loops node
    [[ $(stat -Lc '%d:%i' "$windows_mount_disk") == "$windows_mount_identity" ]] || {
        echo 'Windows disk identity changed; refusing loop cleanup' >&2
        return 1
    }
    loops=$(losetup --list --associated "$windows_mount_disk" --noheadings --output NAME) || return 1
    if [[ -z "$loops" ]]; then
        [[ -z "$windows_mount_loop" ]] || {
            echo 'Previously returned Windows loop disappeared; refusing detach' >&2
            return 1
        }
        return 0
    fi
    if [[ ! "$loops" =~ ^/dev/loop[0-9]+$ ]]; then
        echo 'Cannot establish unique Windows loop ownership; refusing detach' >&2
        return 1
    fi
    node=/sys/block/${loops##*/}
    if [[ ( -z "$windows_mount_loop" || "$windows_mount_loop" == "$loops" ) &&
        $(cat "$node/loop/backing_file") == "$windows_mount_disk" &&
        $(cat "$node/ro") == 1 &&
        $(cat "$node/loop/offset") == "$windows_mount_offset" &&
        $(cat "$node/loop/sizelimit") == "$windows_mount_size" &&
        $(cat "$node/loop/partscan") == "$windows_mount_partscan" ]]; then
        printf '%s\n' "$loops"
    else
        echo 'Windows loop backing or parameters differ; refusing detach' >&2
        return 1
    fi
}

windows_mount_attach() {
    local offset=${2-} size=${3:-0} existing observed
    local arguments=(--find --show --read-only)
    [[ $# == 2 || $# == 3 ]] || return 1
    [[ "$windows_mount_attempted" == 0 ]] || return 1
    windows_mount_require_unmounted "$windows_mount_directory" || return
    [[ "$size" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
    windows_mount_disk=$(realpath "$1") || return
    [[ -f "$windows_mount_disk" ]] || return 1
    exec 9<"$windows_mount_disk"
    flock -n 9 || { echo 'Windows disk verification is already locked' >&2; return 1; }
    windows_mount_identity=$(stat -Lc '%d:%i' /proc/self/fd/9) || return
    [[ $(stat -Lc '%d:%i' "$windows_mount_disk") == "$windows_mount_identity" ]] || return 1
    windows_mount_size=$size
    if [[ -n "$offset" ]]; then
        [[ "$offset" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
        windows_mount_offset=$offset
        arguments+=(--offset "$offset")
    else
        [[ "$size" == 0 ]] || return 1
        windows_mount_partscan=1
        arguments+=(--partscan)
    fi
    if [[ "$size" != 0 ]]; then arguments+=(--sizelimit "$size"); fi
    existing=$(losetup --list --associated "$windows_mount_disk" --noheadings --output NAME) || return
    [[ -z "$existing" ]] || { echo 'Windows disk already has a loop association' >&2; return 1; }
    windows_mount_attempted=1
    windows_mount_loop=$(losetup "${arguments[@]}" "$windows_mount_disk") || return
    observed=$(windows_mount_observe) || return
    [[ -n "$observed" && "$observed" == "$windows_mount_loop" ]] || {
        echo 'losetup did not return the observed Windows loop' >&2
        return 1
    }
}

windows_mount_detach() {
    local observed remaining retry
    [[ "$windows_mount_attempted" == 1 ]] || return 0
    observed=$(windows_mount_observe) || return
    [[ -n "$observed" ]] || return 0
    losetup -d "$observed" || return
    for ((retry = 0; retry < 50; retry++)); do
        [[ $(stat -Lc '%d:%i' "$windows_mount_disk") == "$windows_mount_identity" ]] || {
            echo 'Windows disk identity changed after detach' >&2
            return 1
        }
        remaining=$(losetup --list --associated "$windows_mount_disk" --noheadings --output NAME) || return
        [[ -n "$remaining" ]] || return 0
        [[ "$remaining" == "$observed" ]] || {
            echo 'Windows loop association changed after detach' >&2
            return 1
        }
        sleep 0.1
    done
    echo 'Windows loop association remains after detach' >&2
    return 1
}

windows_mount_cleanup() {
    local result=$?
    trap - EXIT
    trap '' INT TERM
    if [[ "$windows_mount_attempted" == 1 ]]; then
        windows_mount_unmount "$windows_mount_directory" umount || { if [[ "$result" = 0 ]]; then result=1; fi; }
        windows_mount_detach || { if [[ "$result" = 0 ]]; then result=1; fi; }
    fi
    exit "$result"
}
trap windows_mount_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
