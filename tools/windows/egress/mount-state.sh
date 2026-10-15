#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
# Sourced before creating the offline verification mount and loop device.
windows_mount_directory=
windows_mount_loop=
windows_mount_cleanup() {
    local result=$?
    trap - EXIT
    if [[ -n "$windows_mount_directory" ]] && mountpoint -q "$windows_mount_directory"; then
        umount "$windows_mount_directory" || { if [[ "$result" = 0 ]]; then result=1; fi; }
    fi
    if [[ -n "$windows_mount_loop" ]]; then
        losetup -d "$windows_mount_loop" || { if [[ "$result" = 0 ]]; then result=1; fi; }
    fi
    exit "$result"
}
trap windows_mount_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
