#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
helper=$(realpath "$(dirname "$0")/mount-state.sh")
fixture=${1:?Set a new absolute workspace fixture directory}
[[ "$fixture" = /* ]]
mkdir "$fixture"
trap 'rm -r "$fixture"' EXIT
: > "$fixture/disk"
cat > "$fixture/commands.sh" <<'SCRIPT'
mountpoint() {
    if [[ $SCENARIO == before-query-error || ( $SCENARIO == mount-query-error && -s $LOOPS ) ]]; then return 1; fi
    if [[ $SCENARIO == already-mounted || ( $MOUNTED == 1 && -s $LOOPS && ! -e $UNMOUNTED ) ]]; then return 0; fi
    return 32
}
umount() {
    echo unmount >> "$CALLS"
    if [[ $UNMOUNT_RESULT == 0 && $SCENARIO != mount-remains ]]; then : > "$UNMOUNTED"; fi
    return "$UNMOUNT_RESULT"
}
sleep() { :; }
fusermount() { umount "$@"; }
flock() { test "$SCENARIO" != lock-failure; }
stat() {
    if [[ $SCENARIO == replaced && -s $LOOPS && ${!#} != /proc/self/fd/9 ]]; then
        echo 2:3
    else echo 1:2
    fi
}
cat() {
    case "$1" in
        /sys/block/loop41/loop/backing_file) echo "$DISK" ;;
        /sys/block/loop41/ro) if [[ $SCENARIO == writable ]]; then echo 0; else echo 1; fi ;;
        /sys/block/loop41/loop/offset) if [[ $SCENARIO == wrong-offset ]]; then echo 99; else echo "${OFFSET:-0}"; fi ;;
        /sys/block/loop41/loop/sizelimit) if [[ $SCENARIO == wrong-size ]]; then echo 99; else echo "$SIZE"; fi ;;
        /sys/block/loop41/loop/partscan) if [[ $SCENARIO == wrong-partscan ]]; then echo 99; elif [[ -z $OFFSET ]]; then echo 1; else echo 0; fi ;;
        *) /bin/cat "$@" ;;
    esac
}
losetup() {
    if [[ $1 == --list ]]; then
        if [[ -e $DETACH_PENDING && $SCENARIO == detach-query-error ]]; then return 42; fi
        /bin/cat "$LOOPS"
        if [[ -e $DETACH_PENDING && $SCENARIO == deferred-once ]]; then : > "$LOOPS"; fi
        return
    fi
    if [[ $1 == -d ]]; then
        [[ $# == 2 && $2 == /dev/loop41 ]] || return 97
        echo detach >> "$CALLS"
        if [[ $DETACH_RESULT == 0 ]]; then
            case "$SCENARIO" in
                deferred-once|deferred-timeout|detach-query-error) : > "$DETACH_PENDING" ;;
                *) : > "$LOOPS" ;;
            esac
        fi
        return "$DETACH_RESULT"
    fi
    echo attach >> "$CALLS"
    printf '%s\n' "$*" > "$ARGUMENTS"
    if [[ $SCENARIO == before-attach-failure ]]; then return 42; fi
    printf '/dev/loop41\n' > "$LOOPS"
    if [[ $SCENARIO == ambiguous ]]; then printf '/dev/loop42\n' >> "$LOOPS"; fi
    if [[ $SCENARIO == lost-output-failure ]]; then return 42; fi
    if [[ $SCENARIO == lost-output-int ]]; then kill -INT "$$"; return 42; fi
    if [[ $SCENARIO == lost-output-term ]]; then kill -TERM "$$"; return 42; fi
    if [[ $SCENARIO == lost-output-zero ]]; then return 0; fi
    if [[ $SCENARIO == wrong-returned ]]; then echo /dev/loop42; else echo /dev/loop41; fi
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
awk '/^cleanup\(\) \{/ {copy=1} copy {print} copy && /^}$/ {copy=0}' "$(dirname "$0")/bitlocker-verify.sh" > "$fixture/bitlocker-cleanup.sh"
grep -q '^cleanup() {' "$fixture/bitlocker-cleanup.sh"
cat > "$fixture/check.sh" <<'SCRIPT'
set -euo pipefail
source "$COMMANDS"
source "$HELPER"
windows_mount_directory=/test/mount
if [[ $MODE == bitlocker ]]; then
    source "$BITLOCKER_CLEANUP"
    state=/test/mount
    trap cleanup EXIT
fi
windows_mount_attach "$DISK" "$OFFSET" "$SIZE"
source "$ACTION_SCRIPT"
SCRIPT
check() {
    local name=$1 action=$2 mounted=$3 unmount_result=$4 detach_result=$5 expected=$6 calls=$7
    local scenario=${8-normal} offset=${9-32256} size=${10-0} result=0
    : > "$fixture/calls"
    : > "$fixture/loops"
    rm -f "$fixture/unmounted" "$fixture/detach-pending"
    if [[ $scenario == preexisting ]]; then echo /dev/loop41 > "$fixture/loops"; fi
    HELPER=$helper COMMANDS=$fixture/commands.sh ACTION_SCRIPT=$fixture/action.sh \
        BITLOCKER_CLEANUP=$fixture/bitlocker-cleanup.sh MODE=$mode \
        CALLS=$fixture/calls LOOPS=$fixture/loops ARGUMENTS=$fixture/arguments \
        UNMOUNTED=$fixture/unmounted DETACH_PENDING=$fixture/detach-pending \
        DISK=$fixture/disk OFFSET=$offset SIZE=$size ACTION=$action MOUNTED=$mounted \
        SCENARIO=$scenario UNMOUNT_RESULT=$unmount_result DETACH_RESULT=$detach_result \
        bash "$fixture/check.sh" || result=$?
    test "$result" = "$expected"
    printf '%b' "$calls" > "$fixture/expected"
    cmp "$fixture/expected" "$fixture/calls"
    if [[ $calls == *detach* && $detach_result == 0 ]]; then
        case "$scenario" in
            deferred-timeout|detach-query-error) test -s "$fixture/loops" ;;
            *) test ! -s "$fixture/loops" ;;
        esac
    fi
    if [[ $calls == attach* ]]; then
        grep -q -- '--read-only' "$fixture/arguments"
        if [[ -z $offset ]]; then grep -q -- '--partscan' "$fixture/arguments"; fi
        if [[ $size != 0 ]]; then grep -q -- "--sizelimit $size" "$fixture/arguments"; fi
    fi
    echo "$mode $name: expected exit $expected and owned cleanup calls"
}
for mode in shared bitlocker; do
    check success success 1 0 0 0 'attach\nunmount\ndetach\n'
    check failure failure 1 0 0 42 'attach\nunmount\ndetach\n'
    check failed-unmount success 1 1 0 1 'attach\nunmount\ndetach\n'
    check failed-detach success 1 0 1 1 'attach\nunmount\ndetach\n'
    check failed-both success 1 1 1 1 'attach\nunmount\ndetach\n'
    check original-failure failure 1 1 1 42 'attach\nunmount\ndetach\n'
    check unmounted success 0 0 0 0 'attach\ndetach\n'
    check interrupted int 1 0 0 130 'attach\nunmount\ndetach\n'
    check terminated term 1 0 0 143 'attach\nunmount\ndetach\n'
    check lost-failure success 0 0 0 42 'attach\ndetach\n' lost-output-failure
    check lost-int success 0 0 0 130 'attach\ndetach\n' lost-output-int
    check lost-term success 0 0 0 143 'attach\ndetach\n' lost-output-term
    check lost-zero success 0 0 0 1 'attach\ndetach\n' lost-output-zero
    check no-allocation success 0 0 0 42 'attach\n' before-attach-failure
    check prior-association success 0 0 0 1 '' preexisting
    check locked success 1 0 0 1 '' lock-failure
    for scenario in ambiguous writable wrong-offset wrong-size wrong-partscan wrong-returned replaced; do
        check "$scenario" success 0 0 0 1 'attach\n' "$scenario"
    done
    check already-mounted success 0 0 0 1 '' already-mounted
    check query-before success 0 0 0 1 '' before-query-error
    check query-cleanup success 1 0 0 1 'attach\ndetach\n' mount-query-error
    check query-original failure 1 0 0 42 'attach\ndetach\n' mount-query-error
    check mount-remains success 1 0 0 1 'attach\nunmount\ndetach\n' mount-remains
    check deferred success 1 0 0 0 'attach\nunmount\ndetach\n' deferred-once
    check detach-timeout success 1 0 0 1 'attach\nunmount\ndetach\n' deferred-timeout
    check detach-original failure 1 0 0 42 'attach\nunmount\ndetach\n' deferred-timeout
    check detach-query success 1 0 0 1 'attach\nunmount\ndetach\n' detach-query-error
    check partscan success 0 0 0 0 'attach\ndetach\n' normal ''
    check sized-offset success 0 0 0 0 'attach\ndetach\n' normal 32256 1048576
done
