#!/bin/bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail
script=$(realpath "$0")
export WINDOWS_DISK_GUARD=${WINDOWS_DISK_GUARD:-$(realpath "$(dirname "$script")/../egress/assert-disk-closed.sh")}
export WINDOWS_SECURE_HELPER=${WINDOWS_SECURE_HELPER:-$(realpath "$(dirname "$WINDOWS_DISK_GUARD")/secure-state.sh")}
generation=${WINDOWS_GENERATION_CHECKS:-0}
[[ "$generation" = 0 || "$generation" = 1 ]]
if [[ "$generation" = 1 ]]; then
 test "${WINDOWS_CONNECT_TAG:-LEGACY}" = XPLEGACY
 export WINDOWS_GENERATION_READER_GUARD=${WINDOWS_GENERATION_READER_GUARD:-$(realpath "$(dirname "$script")/../generation/check-reader.sh")}
 export WINDOWS_GENERATION_READER_VERIFY=${WINDOWS_GENERATION_READER_VERIFY:-$(realpath "$(dirname "$script")/../generation/verify.py")}
 export WINDOWS_GENERATION_MOUNT_HELPER=${WINDOWS_GENERATION_MOUNT_HELPER:-$(realpath "$(dirname "$script")/../egress/mount-state.sh")}
fi
cd "${WINDOWS_CONNECT_BASE:?Set the test directory containing config.json}"
if [[ ${WINDOWS_CONNECT_RUNNING:-} != 1 ]]; then
 mkdir run
 cp "$script" run/controller.sh
 cp "$WINDOWS_SECURE_HELPER" run/secure-state.sh
 cp "$WINDOWS_DISK_GUARD" run/assert-disk-closed.sh
 chmod 400 run/controller.sh run/secure-state.sh run/assert-disk-closed.sh
 export WINDOWS_SECURE_HELPER=$PWD/run/secure-state.sh WINDOWS_DISK_GUARD=$PWD/run/assert-disk-closed.sh
 if [[ "$generation" = 1 ]]; then
  cp "$WINDOWS_GENERATION_READER_GUARD" run/check-reader.sh
  cp "$WINDOWS_GENERATION_READER_VERIFY" run/verify-generation.py
  cp "$WINDOWS_GENERATION_MOUNT_HELPER" run/mount-state.sh
  chmod 400 run/check-reader.sh run/verify-generation.py run/mount-state.sh
  export WINDOWS_GENERATION_READER_GUARD=$PWD/run/check-reader.sh
  export WINDOWS_GENERATION_READER_VERIFY=$PWD/run/verify-generation.py
  export WINDOWS_GENERATION_MOUNT_HELPER=$PWD/run/mount-state.sh
 fi
 export WINDOWS_CONNECT_RUNNING=1
 exec bash "$PWD/run/controller.sh"
fi
fc=${WINDOWS_CONNECT_FC:-./firecracker}
proxy=${WINDOWS_CONNECT_PROXY:-../windows_probe}
proxy_root=${WINDOWS_CONNECT_PROXY_ROOT:-$PWD/proxy}
sidecar=${WINDOWS_CONNECT_SIDECAR:-$PWD/../tokoloshe-egress}
snapshot_editor=${WINDOWS_CONNECT_SNAPSHOT_EDITOR:?Set an absolute snapshot-editor path}
test -x "$snapshot_editor"
uid=${WINDOWS_CONNECT_UID:-$(id -u)}
gid=${WINDOWS_CONNECT_GID:-$(id -g)}
tag=${WINDOWS_CONNECT_TAG:-LEGACY}
limit=${WINDOWS_CONNECT_ATTEMPTS:-2400}
sudo bash "$WINDOWS_DISK_GUARD" --config config.json
if [[ "$generation" = 1 ]]; then
 reader_identity=$(sudo env WINDOWS_DISK_GUARD="$WINDOWS_DISK_GUARD" \
     WINDOWS_GENERATION_READER_VERIFY="$WINDOWS_GENERATION_READER_VERIFY" \
     WINDOWS_GENERATION_MOUNT_HELPER="$WINDOWS_GENERATION_MOUNT_HELPER" \
     bash "$WINDOWS_GENERATION_READER_GUARD" "$PWD/config.json" "${WINDOWS_CONNECT_DISK:?Set the XP OS disk}" \
     "${WINDOWS_CONNECT_OFFSET:?Set the XP OS partition offset}" xp-connect.exe \
     "${WINDOWS_GENERATION_PROBE_SHA256:?Set the staged XP reader SHA256}")
 printf '%s\n' "$reader_identity" > run/generation-reader-before.json
fi
run=run
# shellcheck source=../egress/secure-state.sh
source "$WINDOWS_SECURE_HELPER"
run_as=()
if [[ -n ${WINDOWS_SECURE_SOURCE:-} ]]; then
 test "$uid" = "$(id -u)"
 test "$gid" = "$(id -g)"
fi
if [[ "$uid" != "$(id -u)" || "$gid" != "$(id -g)" ]]; then
 run_as=(setpriv --reuid "$uid" --regid "$gid" --init-groups)
fi
mkfifo run/proxy-input
exec 3<>run/proxy-input
setsid "${run_as[@]}" "$proxy" "$proxy_root" "$sidecar" "$PWD/vsock.sock_4750" 127.0.0.1:40567 <run/proxy-input >run/proxy.log 2>&1 &
proxy_child=$!
child=
remove_sockets() {
 local result=0 code
 rm -f vsock.sock || result=$?
 if [[ -S console.sock ]]; then
  unlink console.sock || { code=$?; if [[ "$result" == 0 ]]; then result=$code; fi; }
 fi
 return "$result"
}
wait_for_exit() {
 local pid=$1 code
 if ! secure_wait_stopped "$pid"; then
  kill -KILL "$pid" 2>/dev/null || {
   code=$?
   if kill -0 "$pid" 2>/dev/null; then return "$code"; fi
  }
  if ! secure_wait_stopped "$pid"; then return 1; fi
 fi
 wait "$pid"
}
stop_vm() {
 local result=0 code=0
 if [[ -n "$child" ]]; then
  if kill -0 "$child" 2>/dev/null; then
   kill -TERM "$child" || {
    code=$?
    if kill -0 "$child" 2>/dev/null; then result=$code; fi
   }
  fi
  code=0
  wait_for_exit "$child" || code=$?
  if [[ "$code" != 0 && "$code" != 143 && "$result" == 0 ]]; then result=$code; fi
  if kill -0 "$child" 2>/dev/null; then
   if [[ "$result" == 0 ]]; then result=1; fi
  else
   child=
   remove_sockets || { code=$?; if [[ "$result" == 0 ]]; then result=$code; fi; }
  fi
 fi
 return "$result"
}
cleanup_failed() {
 printf 'CONNECT cleanup failed: %s (exit=%s)\n' "$1" "$2" >&2 || true
 if [[ "$result" == 0 ]]; then result=$2; fi
}
cleanup() {
 local result=$? code signal
 trap - EXIT
 trap '' INT TERM
 stop_vm || cleanup_failed stop-vm "$?"
 secure_stop || cleanup_failed secure-stop "$?"
 if kill -0 "$proxy_child" 2>/dev/null; then
  printf '\n' >&3 || cleanup_failed close-proxy-input "$?"
 fi
 wait_for_exit "$proxy_child" || cleanup_failed wait-proxy "$?"
 for signal in TERM KILL; do
  if kill -0 -- "-$proxy_child" 2>/dev/null; then
   if kill -"$signal" -- "-$proxy_child" 2>/dev/null; then
    if [[ "$signal" == KILL ]]; then cleanup_failed forced-proxy-group-stop 1; fi
   else
    code=$?
    if kill -0 -- "-$proxy_child" 2>/dev/null; then cleanup_failed "proxy-group-$signal" "$code"; fi
   fi
  fi
 done
 if kill -0 "$proxy_child" 2>/dev/null || kill -0 -- "-$proxy_child" 2>/dev/null; then
  cleanup_failed live-proxy 1
 else
  rm -f run/proxy-input || cleanup_failed remove-proxy-input "$?"
 fi
 exit "$result"
}
trap 'cleanup' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for retry in {1..30}; do
 if grep -q READY run/proxy.log; then break; fi
 kill -0 "$proxy_child"; sleep 1
done
grep READY run/proxy.log
start() {
 phase=$1; shift
 secure_start "$phase" "$PWD/config.json"
 if [[ "$phase" = cold ]]; then set -- --config-file "$secure_config"; fi
 watermark=$(wc -l < run/proxy.log)
 printf "%s phase=%s watermark=%s\n" "$(date -u +%FT%TZ)" "$phase" "$watermark" >> run/stages.log
 socket=run/$phase.sock
 "${run_as[@]}" "$fc" --enable-pci --api-sock "$socket" "$@" <"${WINDOWS_CONNECT_INPUT:-/dev/null}" >"run/$phase.log" 2>&1 &
 child=$!
 for retry in {1..30}; do
  if test -S "$socket"; then return; fi
  kill -0 "$child"; sleep 1
 done
 return 1
}
request() { curl --connect-timeout 5 --max-time 120 --fail-with-body -sS --unix-socket "$socket" -X "$1" "http://localhost/$2" -H 'Content-Type: application/json' -d "$3"; }
progress() {
 for ((retry = 0; retry < limit; retry++)); do
  count=$(tail -n +"$((watermark + 1))" run/proxy.log | grep -c 'egress CONNECT rejected' || true)
  if (( count > 1 )); then echo "Unexpected future CONNECT stage" >&2; return 1; fi
  if (( count == 1 )); then sleep 5; echo "$tag stage $1 proxy rejection observed; guest transcript verification pending"; return; fi
  kill -0 "$child"; kill -0 "$proxy_child"; sleep 1
 done
 echo "Timed out waiting for $tag CONNECT iteration=$1" >&2
 return 1
}
stage_guard() {
 test "$(grep -c 'egress CONNECT rejected' run/proxy.log || true)" = "$1"
 printf '%s phase=%s completed=%s\n' "$(date -u +%FT%TZ)" "$phase" "$1" >> run/stages.log
}
chown "$uid:$gid" run
start cold --config-file config.json
progress 1
request PATCH vm '{"state":"Paused"}'
stage_guard 1
request PUT snapshot/create '{"snapshot_type":"Full","snapshot_path":"run/full.vm","mem_file_path":"run/full.mem"}'
stop_vm
secure_stop
stage_guard 1
start full
request PUT snapshot/load "$(secure_load '{"snapshot_path":"run/full.vm","mem_backend":{"backend_type":"File","backend_path":"run/full.mem"},"track_dirty_pages":true,"resume_vm":true}')"
progress 2
request PATCH vm '{"state":"Paused"}'
stage_guard 2
request PUT snapshot/create '{"snapshot_type":"Diff","snapshot_path":"run/diff.vm","mem_file_path":"run/diff.mem"}'
stop_vm
secure_stop
"$snapshot_editor" edit-memory rebase --memory-path run/full.mem --diff-path run/diff.mem
stage_guard 2
start diff
request PUT snapshot/load "$(secure_load '{"snapshot_path":"run/diff.vm","mem_backend":{"backend_type":"File","backend_path":"run/full.mem"},"track_dirty_pages":true,"resume_vm":true}')"
progress 3
stage_guard 3
for retry in {1..300}; do
 if ! kill -0 "$child" 2>/dev/null; then wait "$child"; child=; remove_sockets; echo "$tag guest exited 0; inspect transcript and accounting"; exit 0; fi
 sleep 1
done
exit 1
