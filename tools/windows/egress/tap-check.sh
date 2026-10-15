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
if [[ ${WINDOWS_CHECK_RUNNING:-} != 1 ]]; then
    mkdir "$run"
    cp "$0" "$run/controller.sh"
    cp "$WINDOWS_SECURE_HELPER" "$run/secure-state.sh"
    cp "$WINDOWS_DISK_GUARD" "$run/assert-disk-closed.sh"
    chmod 400 "$run/controller.sh" "$run/secure-state.sh" "$run/assert-disk-closed.sh"
    export WINDOWS_SECURE_HELPER=$run/secure-state.sh WINDOWS_DISK_GUARD=$run/assert-disk-closed.sh
    export WINDOWS_CHECK_RUNNING=1
    exec bash "$run/controller.sh"
fi
fc=${WINDOWS_CHECK_FC:?Set an absolute Firecracker binary path}
rebase=${WINDOWS_CHECK_REBASE:?Set an absolute rebase-snap path}
server=${WINDOWS_CHECK_SERVER:?Set an absolute echo server path}
namespace=${WINDOWS_CHECK_NAMESPACE:?Set an existing network namespace}
address=${WINDOWS_CHECK_ADDRESS:?Set the host IPv4 listen address}
test -x "$rebase"
sudo bash "$WINDOWS_DISK_GUARD" --config "$base/config.json"
# shellcheck source=secure-state.sh
source "$WINDOWS_SECURE_HELPER"
child=
echo_child=
stop() {
    if [[ -n "$child" ]]; then
        if kill -0 "$child" 2>/dev/null; then sudo kill -TERM "$(sudo cat "$run/pid")"; fi
        for retry in {1..100}; do
            if ! kill -0 "$child" 2>/dev/null; then break; fi
            sleep 0.1
        done
        if kill -0 "$child" 2>/dev/null; then
            sudo kill -KILL "$(sudo cat "$run/pid")"
            wait "$child" || true
            child=
            return 1
        fi
        local result=0
        wait "$child" || result=$?
        if [[ "$result" != 0 && "$result" != 143 ]]; then return "$result"; fi
        child=
        sudo rm -f "$base/console.sock"
    fi
}
cleanup() {
    local result=$?
    trap - EXIT
    stop || result=$?
    secure_stop || result=$?
    if [[ "$result" != 0 ]]; then secure_discard; fi
    if [[ -n "$echo_child" ]]; then kill "$echo_child" 2>/dev/null || true; wait "$echo_child" || true; fi
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
        if tail -n +"$(( ${watermark:-0} + 1 ))" "$source" | grep -q "$marker"; then
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
request PUT snapshot/load "$(secure_load "$(jq -nc --arg snapshot_path "$run/full.vm" --arg backend_path "$run/full.mem" '{snapshot_path:$snapshot_path,mem_backend:{backend_type:"File",backend_path:$backend_path},enable_diff_snapshots:true,resume_vm:true}')")"
progress 2
request PATCH vm '{"state":"Paused"}'
stage_guard 2
request PUT snapshot/create "$(jq -nc --arg snapshot_path "$run/diff.vm" --arg mem_file_path "$run/diff.mem" '{snapshot_type:"Diff",snapshot_path:$snapshot_path,mem_file_path:$mem_file_path}')"
stop
secure_stop
sudo "$rebase" --base-file "$run/full.mem" --diff-file "$run/diff.mem"
stage_guard 2
start diff
request PUT snapshot/load "$(secure_load "$(jq -nc --arg snapshot_path "$run/diff.vm" --arg backend_path "$run/full.mem" '{snapshot_path:$snapshot_path,mem_backend:{backend_type:"File",backend_path:$backend_path},enable_diff_snapshots:true,resume_vm:true}')")"
progress 3
stage_guard 3
for retry in {1..300}; do
    if ! kill -0 "$child" 2>/dev/null; then wait "$child"; child=; sudo rm -f "$base/console.sock"; echo "$tag full/diff storage and TAP PASS; guest exit 0"; exit 0; fi
    sleep 1
done
exit 1
