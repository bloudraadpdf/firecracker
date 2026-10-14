#!/bin/bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail
script=$(realpath "$0")
export WINDOWS_DISK_GUARD=${WINDOWS_DISK_GUARD:-$(realpath "$(dirname "$script")/../egress/assert-disk-closed.sh")}
export WINDOWS_SECURE_HELPER=${WINDOWS_SECURE_HELPER:-$(realpath "$(dirname "$WINDOWS_DISK_GUARD")/secure-state.sh")}
cd "${WINDOWS_CONNECT_BASE:?Set the test directory containing config.json}"
if [[ ${WINDOWS_CONNECT_RUNNING:-} != 1 ]]; then
 mkdir run
 cp "$script" run/controller.sh
 cp "$WINDOWS_SECURE_HELPER" run/secure-state.sh
 cp "$WINDOWS_DISK_GUARD" run/assert-disk-closed.sh
 chmod 400 run/controller.sh run/secure-state.sh run/assert-disk-closed.sh
 export WINDOWS_SECURE_HELPER=$PWD/run/secure-state.sh WINDOWS_DISK_GUARD=$PWD/run/assert-disk-closed.sh
 export WINDOWS_CONNECT_RUNNING=1
 exec bash "$PWD/run/controller.sh"
fi
fc=${WINDOWS_CONNECT_FC:-./firecracker}
proxy=${WINDOWS_CONNECT_PROXY:-../windows_probe}
proxy_root=${WINDOWS_CONNECT_PROXY_ROOT:-$PWD/proxy}
sidecar=${WINDOWS_CONNECT_SIDECAR:-$PWD/../tokoloshe-egress}
rebase=${WINDOWS_CONNECT_REBASE:-../rebase-snap}
uid=${WINDOWS_CONNECT_UID:-$(id -u)}
gid=${WINDOWS_CONNECT_GID:-$(id -g)}
tag=${WINDOWS_CONNECT_TAG:-LEGACY}
limit=${WINDOWS_CONNECT_ATTEMPTS:-2400}
sudo bash "$WINDOWS_DISK_GUARD" --config config.json
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
 rm -f vsock.sock
 if [[ -S console.sock ]]; then unlink console.sock; fi
}
wait_for_exit() {
 local pid=$1
 for retry in {1..100}; do
  if ! kill -0 "$pid" 2>/dev/null; then wait "$pid"; return; fi
  sleep 0.1
 done
 kill -KILL "$pid" 2>/dev/null || true
 wait "$pid"
}
stop_vm() {
 if [[ -n "$child" ]]; then
  if kill -0 "$child" 2>/dev/null; then kill -TERM "$child"; fi
  code=0
  wait_for_exit "$child" || code=$?
  child=
  remove_sockets
  if [[ "$code" != 0 && "$code" != 143 ]]; then return "$code"; fi
 fi
}
cleanup() {
 result=$?
 trap - EXIT
 stop_vm || result=$?
 secure_stop || result=$?
 if [[ "$result" != 0 ]]; then secure_discard; fi
 if kill -0 "$proxy_child" 2>/dev/null; then
  printf '\n' >&3
 fi
 wait_for_exit "$proxy_child" || result=$?
 kill -TERM -- "-$proxy_child" 2>/dev/null || true
 kill -KILL -- "-$proxy_child" 2>/dev/null || true
 rm -f run/proxy-input
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
request PUT snapshot/load "$(secure_load '{"snapshot_path":"run/full.vm","mem_backend":{"backend_type":"File","backend_path":"run/full.mem"},"enable_diff_snapshots":true,"resume_vm":true}')"
progress 2
request PATCH vm '{"state":"Paused"}'
stage_guard 2
request PUT snapshot/create '{"snapshot_type":"Diff","snapshot_path":"run/diff.vm","mem_file_path":"run/diff.mem"}'
stop_vm
secure_stop
"$rebase" --base-file run/full.mem --diff-file run/diff.mem
stage_guard 2
start diff
request PUT snapshot/load "$(secure_load '{"snapshot_path":"run/diff.vm","mem_backend":{"backend_type":"File","backend_path":"run/full.mem"},"enable_diff_snapshots":true,"resume_vm":true}')"
progress 3
stage_guard 3
for retry in {1..300}; do
 if ! kill -0 "$child" 2>/dev/null; then wait "$child"; child=; remove_sockets; echo "$tag guest exited 0; inspect transcript and accounting"; exit 0; fi
 sleep 1
done
exit 1
