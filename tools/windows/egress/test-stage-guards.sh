#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
fixture=${1:?Set a new absolute workspace fixture directory}
[[ "$fixture" = /* ]]
mkdir "$fixture"
mkdir "$fixture/run"
trap 'rm -r "$fixture"' EXIT
printf '%s\n' 'echo: peer=172.30.254.2:1 bytes=2097152' 'report: PASS WIN7_NET iteration=1 done' 'echo: peer=172.30.254.2:2 bytes=2097152' 'report: PASS WIN7_NET iteration=2 done' > "$fixture/echo.log"
printf '%s\n' 'egress CONNECT rejected' 'egress CONNECT rejected' > "$fixture/run/proxy.log"
printf '%s\n' 'no post-restore egress' > "$fixture/full.log"
for controller in tap connect; do
    source_file=$root/tools/windows/egress/tap-check.sh
    if [[ "$controller" = connect ]]; then source_file=$root/tools/windows/legacy-vsock/check.sh; fi
    functions=$fixture/$controller.functions
    awk '/^(progress|stage_guard)\(\) \{/ {copy=1} copy {print} copy && /^}$/ {copy=0}' "$source_file" > "$functions"
    grep -q '^progress() {' "$functions"
    grep -q '^stage_guard() {' "$functions"
    for scenario in stale excess fresh exact large-fresh; do
        if [[ "$scenario" = large-fresh ]]; then
            if [[ "$controller" = connect ]]; then continue; fi
            printf '%262144s\n' '' >> "$fixture/echo.log"
        fi
        script=$fixture/$controller-$scenario.sh
        {
            printf 'set -euo pipefail\ncd %q\nrun=%q\n' "$fixture" "$fixture"
            printf 'tag=WIN7\nphase=full\nlimit=1\nchild=$$\necho_child=$$\nproxy_child=$$\n'
            if [[ "$scenario" = fresh || "$scenario" = large-fresh ]]; then
                if [[ "$controller" = tap ]]; then printf 'watermark=2\n'; else printf 'watermark=1\n'; fi
            elif [[ "$controller" = tap ]]; then printf 'watermark=4\n'; else printf 'watermark=2\n'; fi
            cat "$functions"
            case "$scenario" in
                stale|fresh|large-fresh) printf '\nprogress 2\n' ;;
                excess) printf '\nstage_guard 1\n' ;;
                exact) printf '\nstage_guard 2\n' ;;
            esac
        } > "$script"
        result=0
        bash "$script" > "$fixture/result.log" 2>&1 || result=$?
        expected=1
        if [[ "$scenario" = fresh || "$scenario" = large-fresh || "$scenario" = exact ]]; then expected=0; fi
        if [[ "$result" != "$expected" ]]; then
            cat "$fixture/result.log"
            echo "$controller $scenario: expected exit $expected, got $result" >&2
            exit 1
        fi
        echo "$controller $scenario: expected exit $expected"
    done
done
