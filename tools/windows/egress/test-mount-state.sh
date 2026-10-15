#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
helper=$(realpath "$(dirname "$0")/mount-state.sh")
fixture=${1:?Set a new absolute workspace fixture directory}
[[ "$fixture" = /* ]]
mkdir "$fixture"
trap 'rm -r "$fixture"' EXIT
cat > "$fixture/commands.sh" <<'SCRIPT'
mountpoint() { test "$MOUNTED" = 1; }
umount() { echo unmount >> "$CALLS"; return "$UNMOUNT_RESULT"; }
fusermount() { umount "$@"; }
losetup() {
    test "$1" = -d
    test "$2" = /dev/test-loop
    echo detach >> "$CALLS"
    return "$DETACH_RESULT"
}
SCRIPT
cat > "$fixture/action.sh" <<'SCRIPT'
case "$ACTION" in
    success) exit 0 ;;
    failure) exit 42 ;;
    int) kill -INT "$$" ;;
    term) kill -TERM "$$" ;;
esac
SCRIPT
cat > "$fixture/check.sh" <<'SCRIPT'
set -euo pipefail
source "$COMMANDS"
source "$HELPER"
windows_mount_directory=/test/mount
windows_mount_loop=/dev/test-loop
source "$ACTION_SCRIPT"
SCRIPT
check_script=$fixture/check.sh
check() {
    local name=$1 action=$2 mounted=$3 unmount_result=$4 detach_result=$5 expected=$6 result=0
    : > "$fixture/calls"
    HELPER=$helper COMMANDS=$fixture/commands.sh ACTION_SCRIPT=$fixture/action.sh \
        CALLS=$fixture/calls ACTION=$action MOUNTED=$mounted \
        UNMOUNT_RESULT=$unmount_result DETACH_RESULT=$detach_result \
        bash "$check_script" || result=$?
    test "$result" = "$expected"
    if [[ "$mounted" = 1 ]]; then
        printf 'unmount\ndetach\n' > "$fixture/expected"
    else
        printf 'detach\n' > "$fixture/expected"
    fi
    cmp "$fixture/expected" "$fixture/calls"
    echo "$name: expected exit $expected and complete cleanup"
}
check success success 1 0 0 0
check failure failure 1 0 0 42
check failed-unmount success 1 1 0 1
check failed-detach success 1 0 1 1
check failed-both success 1 1 1 1
check original-failure failure 1 1 1 42
check unmounted success 0 0 0 0
check interrupted int 1 0 0 130
check terminated term 1 0 0 143
awk '/^cleanup\(\) \{/ {copy=1} copy {print} copy && /^}$/ {copy=0}' "$(dirname "$0")/bitlocker-verify.sh" > "$fixture/bitlocker-cleanup.sh"
grep -q '^cleanup() {' "$fixture/bitlocker-cleanup.sh"
helper=$fixture/bitlocker-cleanup.sh
cat > "$fixture/bitlocker-check.sh" <<'SCRIPT'
set -euo pipefail
source "$COMMANDS"
source "$HELPER"
state=/test/mount
source_loop=/dev/test-loop
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
source "$ACTION_SCRIPT"
SCRIPT
check_script=$fixture/bitlocker-check.sh
check bitlocker-success success 1 0 0 0
check bitlocker-unmount success 1 1 0 1
check bitlocker-detach success 1 0 1 1
check bitlocker-both success 1 1 1 1
check bitlocker-original-failure failure 1 1 1 42
check bitlocker-interrupted int 1 1 1 130
check bitlocker-terminated term 1 1 1 143
