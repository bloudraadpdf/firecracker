#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
# Sourced by the TAP and CONNECT controllers.
secure_pids=()
secure_current=
secure_config=
secure_stop() {
    local pid code=0 result=0
    for pid in "${secure_pids[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
    for pid in "${secure_pids[@]}"; do
        for retry in {1..100}; do
            if ! kill -0 "$pid" 2>/dev/null; then break; fi
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; result=1; fi
        code=0
        wait "$pid" || code=$?
        if [[ "$code" != 0 && "$code" != 143 ]]; then result=1; fi
    done
    secure_pids=()
    return "$result"
}
secure_start() {
    local phase=$1 config=$2 source generation stage pid lock LC_ALL=C
    secure_config=$config
    if [[ -z ${WINDOWS_SECURE_SOURCE:-} ]]; then return; fi
    umask 077
    source=${secure_current:-$WINDOWS_SECURE_SOURCE}
    stage=$(realpath "${run:?Set the controller run directory}")/$phase-state
    [[ "$source" = /* ]]
    test -z "${secure_pids[*]}"
    sudo bash "$WINDOWS_DISK_GUARD" "$source/store.json"
    sudo bash "$WINDOWS_DISK_GUARD" "$source/tpm/tpm2-00.permall"
    test "${#stage}" -le 95
    for lock in "$source/store.lock" "$source/tpm/.lock"; do
        if [[ -f "$lock" ]]; then sudo bash "$WINDOWS_DISK_GUARD" "$lock"; fi
    done
    mkdir -m 700 "$stage"
    secure_current=$stage
    mkdir -m 700 "$secure_current/tpm"
    cp "$source/store.json" "$secure_current/store.json"
    find "$source/tpm" -maxdepth 1 -type f ! -name .lock -exec cp '{}' "$secure_current/tpm/" \;
    generation=$(jq -er '.generation | select(type == "number" and . >= 0 and . <= 9007199254740991 and floor == .)' "$secure_current/store.json")
    "${WINDOWS_SECURE_VARS:?Set the variable helper binary}" serve "$secure_current/store.json" "$secure_current/vars.sock" > "$secure_current/vars.log" 2>&1 &
    secure_pids+=("$!")
    "${WINDOWS_SECURE_SWTPM:-swtpm}" socket --tpm2 --tpmstate "dir=$secure_current/tpm" --ctrl "type=unixio,path=$secure_current/tpm/ctrl" --flags not-need-init,disable-auto-shutdown > "$secure_current/tpm.log" 2>&1 &
    secure_pids+=("$!")
    for ((retry = 0; retry < 100; retry++)); do
        if [[ -S "$secure_current/vars.sock" && -S "$secure_current/tpm/ctrl" ]]; then break; fi
        for pid in "${secure_pids[@]}"; do kill -0 "$pid"; done
        sleep 0.1
    done
    test -S "$secure_current/vars.sock"
    test -S "$secure_current/tpm/ctrl"
    jq -nc --arg socket_path "$secure_current/tpm/ctrl" --arg vars "$secure_current/vars.sock" --argjson generation "$generation" '{tpm:{socket_path:$socket_path},uefi_vars:{socket_path:$vars,expected_generation:$generation}}' > "$secure_current/load.json"
    secure_config=$secure_current/config.json
    jq --slurpfile helpers "$secure_current/load.json" '. + {tpm:$helpers[0].tpm,"uefi-vars":{socket_path:$helpers[0].uefi_vars.socket_path}}' "$config" > "$secure_config"
}
secure_load() {
    if [[ -z ${WINDOWS_SECURE_SOURCE:-} ]]; then printf '%s' "$1"; return; fi
    jq -nc --argjson request "$1" --slurpfile helpers "$secure_current/load.json" '$request + $helpers[0]'
}
secure_discard() {
    if [[ -n "$secure_current" ]]; then
        rm -f "$secure_current/store.json" "$secure_current/store.lock"
        rm -rf "$secure_current/tpm"
    fi
}
