#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
export WINDOWS_DISK_GUARD=${WINDOWS_DISK_GUARD:-$(realpath "$(dirname "$0")/assert-disk-closed.sh")}
export WINDOWS_SECURE_HELPER=${WINDOWS_SECURE_HELPER:-$(realpath "$(dirname "$WINDOWS_DISK_GUARD")/secure-state.sh")}
base=${WINDOWS_CHECK_BASE:?Set an absolute test directory}
tag=${WINDOWS_GUEST_TAG:-WIN7}
limit=${WINDOWS_WAIT_SECONDS:-240}
[[ "$base" = /* && "$tag" =~ ^[A-Z0-9_-]+$ ]]
run=$base/${WINDOWS_CHECK_RUN:-run1}
generation=${WINDOWS_GENERATION_CHECKS:-0}
[[ "$generation" = 0 || "$generation" = 1 ]]
if [[ "$generation" = 1 && "$tag" = XP ]]; then
    export WINDOWS_GENERATION_READER_GUARD=${WINDOWS_GENERATION_READER_GUARD:-$(realpath "$(dirname "$0")/../generation/check-reader.sh")}
    export WINDOWS_GENERATION_READER_VERIFY=${WINDOWS_GENERATION_READER_VERIFY:-$(realpath "$(dirname "$0")/../generation/verify.py")}
    export WINDOWS_GENERATION_MOUNT_HELPER=${WINDOWS_GENERATION_MOUNT_HELPER:-$(realpath "$(dirname "$0")/mount-state.sh")}
fi
if [[ ${WINDOWS_CHECK_RUNNING:-} != 1 ]]; then
    mkdir "$run"
    cp "$0" "$run/controller.sh"
    cp "$WINDOWS_SECURE_HELPER" "$run/secure-state.sh"
    cp "$WINDOWS_DISK_GUARD" "$run/assert-disk-closed.sh"
    chmod 400 "$run/controller.sh" "$run/secure-state.sh" "$run/assert-disk-closed.sh"
    export WINDOWS_SECURE_HELPER=$run/secure-state.sh WINDOWS_DISK_GUARD=$run/assert-disk-closed.sh
    if [[ "$generation" = 1 && "$tag" = XP ]]; then
        cp "$WINDOWS_GENERATION_READER_GUARD" "$run/check-reader.sh"
        cp "$WINDOWS_GENERATION_READER_VERIFY" "$run/verify-generation.py"
        cp "$WINDOWS_GENERATION_MOUNT_HELPER" "$run/mount-state.sh"
        chmod 400 "$run/check-reader.sh" "$run/verify-generation.py" "$run/mount-state.sh"
        export WINDOWS_GENERATION_READER_GUARD=$run/check-reader.sh
        export WINDOWS_GENERATION_READER_VERIFY=$run/verify-generation.py
        export WINDOWS_GENERATION_MOUNT_HELPER=$run/mount-state.sh
    fi
    export WINDOWS_CHECK_RUNNING=1
    exec bash "$run/controller.sh"
fi
fc=${WINDOWS_CHECK_FC:?Set an absolute Firecracker binary path}
snapshot_editor=${WINDOWS_CHECK_SNAPSHOT_EDITOR:?Set an absolute snapshot-editor path}
server=${WINDOWS_CHECK_SERVER:?Set an absolute echo server path}
namespace=${WINDOWS_CHECK_NAMESPACE:?Set an existing network namespace}
address=${WINDOWS_CHECK_ADDRESS:?Set the host IPv4 listen address}
test -x "$snapshot_editor"
sudo bash "$WINDOWS_DISK_GUARD" --config "$base/config.json"
if [[ "$generation" = 1 && "$tag" = XP ]]; then
    reader_identity=$(sudo env WINDOWS_DISK_GUARD="$WINDOWS_DISK_GUARD" \
        WINDOWS_GENERATION_READER_VERIFY="$WINDOWS_GENERATION_READER_VERIFY" \
        WINDOWS_GENERATION_MOUNT_HELPER="$WINDOWS_GENERATION_MOUNT_HELPER" \
        bash "$WINDOWS_GENERATION_READER_GUARD" "$base/config.json" "${WINDOWS_CHECK_DISK:?Set the XP OS disk}" \
        "${WINDOWS_PARTITION_OFFSET:?Set the XP OS partition offset}" restore-probe.exe \
        "${WINDOWS_GENERATION_PROBE_SHA256:?Set the staged XP reader SHA256}")
    printf '%s\n' "$reader_identity" > "$run/generation-reader-before.json"
fi
# shellcheck source=secure-state.sh
source "$WINDOWS_SECURE_HELPER"
child=
echo_child=
stop() {
    local result=0 code pid signal
    if [[ -n "$child" ]]; then
        for signal in TERM KILL; do
            if ! kill -0 "$child" 2>/dev/null; then break; fi
            code=0
            pid=$(sudo cat "$run/pid") || code=$?
            if [[ "$code" == 0 && ! "$pid" =~ ^[1-9][0-9]*$ ]]; then code=1; fi
            if [[ "$code" == 0 ]]; then sudo kill -"$signal" "$pid" || code=$?; fi
            if [[ "$code" != 0 ]] && kill -0 "$child" 2>/dev/null; then
                printf 'TAP VMM %s failed (%s)\n' "$signal" "$code" >&2 || true
                if [[ "$result" == 0 ]]; then result=$code; fi
            fi
            if [[ "$signal" == KILL && "$result" == 0 ]]; then result=1; fi
            if secure_wait_stopped "$child"; then break; fi
        done
        if kill -0 "$child" 2>/dev/null; then
            if [[ "$result" == 0 ]]; then result=1; fi
        else
            code=0
            wait "$child" || code=$?
            if [[ "$code" != 0 && "$code" != 143 && "$result" == 0 ]]; then result=$code; fi
            child=
            sudo rm -f "$base/console.sock" || { code=$?; if [[ "$result" == 0 ]]; then result=$code; fi; }
        fi
    fi
    return "$result"
}
cleanup_failed() {
    local code=$1
    printf 'TAP cleanup failed: %s (%s)\n' "$2" "$code" >&2 || true
    if [[ "$result" == 0 ]]; then result=$code; fi
}
cleanup() {
    local result=$? code
    trap - EXIT
    trap '' INT TERM
    stop || cleanup_failed $? VMM
    secure_stop || cleanup_failed $? secure-helpers
    if [[ "$result" != 0 ]]; then secure_discard || cleanup_failed $? secure-state; fi
    if [[ -n "$echo_child" ]]; then
        code=0
        kill -TERM "$echo_child" 2>/dev/null || code=$?
        if [[ "$code" != 0 ]] && kill -0 "$echo_child" 2>/dev/null; then cleanup_failed "$code" echo-TERM; fi
        if ! secure_wait_stopped "$echo_child"; then
            code=0
            kill -KILL "$echo_child" 2>/dev/null || code=$?
            if [[ "$code" != 0 ]] && kill -0 "$echo_child" 2>/dev/null; then cleanup_failed "$code" echo-KILL; fi
            cleanup_failed 1 echo-forced-stop
        fi
        if secure_wait_stopped "$echo_child"; then
            code=0
            wait "$echo_child" || code=$?
            if [[ "$code" != 0 && "$code" != 143 ]]; then cleanup_failed "$code" echo-wait; fi
            echo_child=
        else
            cleanup_failed 1 echo-still-live
        fi
    fi
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$server" "$address" > "$run/echo.log" 2>&1 &
echo_child=$!
start() {
    phase=$1
    watermark=$(wc -l < "$run/echo.log")
    printf "%s phase=%s watermark=%s\n" "$(date -u +%FT%TZ)" "$phase" "$watermark" >> "$run/stages.log"
    shift
    secure_start "$phase" "$base/config.json"
    if [[ "$phase" = cold ]]; then set -- --config-file "$secure_config"; fi
    socket=$run/$phase.sock
    { sudo sh -c 'echo $$ > "$1"; shift; namespace=$1; shift; exec ip netns exec "$namespace" "$@"' sh "$run/pid" "$namespace" "$fc" --enable-pci --api-sock "$socket" "$@"; } < "${WINDOWS_CHECK_INPUT:-/dev/null}" > "$run/$phase.log" 2>&1 &
    child=$!
    for retry in {1..30}; do
        if sudo test -S "$socket"; then return; fi
        kill -0 "$child"
        kill -0 "$echo_child"
        sleep 1
    done
    return 1
}
request() {
    sudo curl --max-time 60 --fail-with-body -sS --unix-socket "$socket" -X "$1" "http://localhost/$2" -H 'Content-Type: application/json' -d "$3"
}
progress() {
    local source="$run/echo.log" marker="PASS ${tag}_NET iteration=$1" failure="FAIL ${tag}_NET"
    if [[ "$tag" = XP ]]; then
        source="$run/$phase.log"
        marker="XP_CHECK iteration=$1"
        failure=XP_CHECK_FAILED
        watermark=0
    fi
    for ((retry = 0; retry < limit; retry++)); do
        if tail -n +"$(( ${watermark:-0} + 1 ))" "$source" | grep "$marker" > /dev/null; then
            grep "$marker" "$source"
            return
        fi
        if grep -q "$failure" "$source"; then
            cat "$source"
            for ((exit_retry = 0; exit_retry < 60; exit_retry++)); do
                if ! kill -0 "$child" 2>/dev/null; then wait "$child" || true; child=; break; fi
                sleep 1
            done
            return 1
        fi
        kill -0 "$child"
        kill -0 "$echo_child"
        sleep 1
    done
    tail -40 "$run/$phase.log"
    return 1
}
stage_guard() {
    local expected=$1 count
    count=$(grep -c '^echo:.* bytes=2097152$' "$run/echo.log" || true)
    test "$count" = "$expected"
    if [[ "$tag" = XP ]]; then
        test "$(grep -c 'XP_CHECK iteration=' "$run/$phase.log" || true)" = 1
        grep -q "XP_CHECK iteration=$expected" "$run/$phase.log"
    else
        test "$(grep -c "^report: PASS ${tag}_NET iteration=" "$run/echo.log" || true)" = "$expected"
    fi
    printf '%s phase=%s completed=%s\n' "$(date -u +%FT%TZ)" "$phase" "$expected" >> "$run/stages.log"
}
start cold --config-file "$base/config.json"
progress 1
request PATCH vm '{"state":"Paused"}'
stage_guard 1
request PUT snapshot/create "$(jq -nc --arg snapshot_path "$run/full.vm" --arg mem_file_path "$run/full.mem" '{snapshot_type:"Full",snapshot_path:$snapshot_path,mem_file_path:$mem_file_path}')"
stop
secure_stop
stage_guard 1
start full
request PUT snapshot/load "$(secure_load "$(jq -nc --arg snapshot_path "$run/full.vm" --arg backend_path "$run/full.mem" '{snapshot_path:$snapshot_path,mem_backend:{backend_type:"File",backend_path:$backend_path},track_dirty_pages:true,resume_vm:true}')")"
progress 2
request PATCH vm '{"state":"Paused"}'
stage_guard 2
request PUT snapshot/create "$(jq -nc --arg snapshot_path "$run/diff.vm" --arg mem_file_path "$run/diff.mem" '{snapshot_type:"Diff",snapshot_path:$snapshot_path,mem_file_path:$mem_file_path}')"
stop
secure_stop
sudo "$snapshot_editor" edit-memory rebase --memory-path "$run/full.mem" --diff-path "$run/diff.mem"
stage_guard 2
start diff
request PUT snapshot/load "$(secure_load "$(jq -nc --arg snapshot_path "$run/diff.vm" --arg backend_path "$run/full.mem" '{snapshot_path:$snapshot_path,mem_backend:{backend_type:"File",backend_path:$backend_path},track_dirty_pages:true,resume_vm:true}')")"
progress 3
stage_guard 3
for retry in {1..300}; do
    if ! kill -0 "$child" 2>/dev/null; then wait "$child"; child=; sudo rm -f "$base/console.sock"; echo "$tag full/diff storage and TAP PASS; guest exit 0"; exit 0; fi
    sleep 1
done
exit 1
