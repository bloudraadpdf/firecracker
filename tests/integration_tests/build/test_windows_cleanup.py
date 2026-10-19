# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check Windows egress controller cleanup without a guest."""

import contextlib
import socket
from subprocess import run

import pytest


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "failure,code",
    [
        ("none", 0),
        ("stop", 7),
        ("secure", 11),
        ("fifo", 19),
        ("wait", 23),
        ("group", 29),
        ("remove", 31),
        ("all", 7),
    ],
)
def test_connect_cleanup_preserves_error_and_attempts_all(
    windows_cleanup, original, failure, code, tmp_path
):
    """A cleanup failure must not replace an original exit or skip later work."""
    statuses = {
        "stop": 7,
        "secure": 11,
        "fifo": 19,
        "wait": 23,
        "group": 29,
        "remove": 31,
    }
    values = {
        key: value if failure in [key, "all"] else 0 for key, value in statuses.items()
    }
    script = f"""set -euo pipefail
stop_vm() {{ echo stop; return {values['stop']}; }}
secure_stop() {{ echo secure; return {values['secure']}; }}
parent_gone=0
group_gone=0
wait_for_exit() {{ echo wait; parent_gone=1; return {values['wait']}; }}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return "$parent_gone"; fi
 echo group-${{1#-}}
 if [[ {values['group']} == 0 ]]; then group_gone=1; fi
 return {values['group']}
}}
printf() {{
 if [[ $1 == '\\n' ]]; then echo fifo >&4; return {values['fifo']}; fi
 builtin printf "$@"
}}
rm() {{ echo remove; return {values['remove']}; }}
proxy_child=47
secure_current=state
secure_pids=()
exec 4>&1
exec 3>/dev/null
{windows_cleanup.get('cleanup_failed', '')}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (original or code), result.stderr
    calls = result.stdout.decode().splitlines()
    expected = ["stop", "secure"]
    expected += ["fifo", "wait", "group-TERM"]
    expected += ["group-KILL"] if values["group"] else ["remove"]
    assert calls == expected
    if failure != "none":
        assert b"CONNECT cleanup failed:" in result.stderr


@pytest.mark.parametrize("remove,unlink", [(0, 0), (7, 0), (0, 11), (7, 11)])
def test_connect_socket_cleanup_keeps_first_failure(
    windows_cleanup, remove, unlink, tmp_path
):
    """A later successful unlink must not hide the first socket removal error."""
    with socket.socket(socket.AF_UNIX) as console:
        with contextlib.chdir(tmp_path):
            console.bind("console.sock")
        script = f"""set -euo pipefail
rm() {{ echo remove; return {remove}; }}
unlink() {{ echo unlink; return {unlink}; }}
{windows_cleanup['remove_sockets']}
result=0
remove_sockets || result=$?
exit "$result"
"""
        result = run(
            ["bash", "-c", script],
            cwd=tmp_path,
            capture_output=True,
            timeout=5,
            check=False,
        )
    assert result.returncode == (remove or unlink)
    assert result.stdout.splitlines() == [b"remove", b"unlink"]


@pytest.mark.parametrize(
    "term,wait,sockets",
    [(0, 0, 0), (7, 0, 0), (0, 143, 0), (0, 137, 0), (0, 0, 11), (7, 137, 11)],
)
def test_connect_stop_keeps_first_failure(
    windows_cleanup, term, wait, sockets, tmp_path
):
    """Stopping a VM retains TERM/wait/socket errors and attempts every step."""
    script = f"""set -euo pipefail
gone=0
kill() {{ if [[ $1 == -0 ]]; then return "$gone"; fi; echo term; return {term}; }}
wait_for_exit() {{ echo wait; gone=1; return {wait}; }}
remove_sockets() {{ echo sockets; return {sockets}; }}
child=41
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (
        term or (wait if wait not in [0, 143] else 0) or sockets
    )
    assert result.stdout.splitlines() == [b"term", b"wait", b"sockets"]


@pytest.mark.parametrize("kill_result,still_alive", [(0, True), (7, True), (7, False)])
def test_connect_wait_reports_failed_kill_of_live_child(
    windows_cleanup, kill_result, still_alive, tmp_path
):
    """A failed forced kill is an error while the child is still observed live."""
    script = f"""set -euo pipefail
forced=0
kill() {{
 if [[ $1 == -KILL ]]; then forced=1; echo kill; return {kill_result}; fi
 if [[ $forced == 1 ]]; then return {0 if still_alive else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo wait; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
result=0
wait_for_exit 41 || result=$?
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    expected = (kill_result or 1) if still_alive else 0
    assert result.returncode == expected
    assert result.stdout.splitlines() == ([b"kill"] if expected else [b"kill", b"wait"])


@pytest.mark.parametrize("still_alive", [False, True])
def test_connect_stop_allows_natural_exit_during_term(
    windows_cleanup, still_alive, tmp_path
):
    """A TERM failure is benign only when the owned VMM is then observed gone."""
    script = f"""set -euo pipefail
signalled=0
reaped=0
kill() {{
 if [[ $1 == -TERM ]]; then signalled=1; echo term; return 7; fi
 if [[ $reaped == 1 ]]; then return 1; fi
 if [[ $signalled == 1 ]]; then return {0 if still_alive else 1}; fi
 return 0
}}
wait_for_exit() {{ echo wait; reaped=1; return 0; }}
remove_sockets() {{ echo sockets; return 0; }}
child=41
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (7 if still_alive else 0)
    assert result.stdout.splitlines() == [b"term", b"wait", b"sockets"]


@pytest.mark.parametrize("disappearance", ["before", "during", "never"])
def test_connect_cleanup_checks_group_signal_races(
    windows_cleanup, disappearance, tmp_path
):
    """A signal failure must not fail cleanup after the owned group disappears."""
    script = f"""set -euo pipefail
stop_vm() {{ :; }}
secure_stop() {{ :; }}
wait_for_exit() {{ :; }}
rm() {{ echo remove; }}
group_gone={1 if disappearance == 'before' else 0}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return 1; fi
 echo signal-${{1#-}}
 {'group_gone=1' if disappearance == 'during' else ':'}
 return 7
}}
proxy_child=47
{windows_cleanup.get('cleanup_failed', '')}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit 0
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (7 if disappearance == "never" else 0)
    expected = [] if disappearance == "before" else [b"signal-TERM"]
    if disappearance == "never":
        expected += [b"signal-KILL"]
    if disappearance != "never":
        expected += [b"remove"]
    assert result.stdout.splitlines() == expected
    assert (b"CONNECT cleanup failed:" in result.stderr) == (disappearance == "never")


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "mode,code",
    [
        ("before", 0),
        ("natural", 0),
        ("forced-gone", 1),
        ("forced-live", 1),
        ("term-race", 0),
        ("kill-race", 0),
        ("term-error", 11),
        ("kill-error", 7),
        ("parent-live", 7),
    ],
)
def test_connect_cleanup_requires_proxy_group_stop(
    windows_cleanup, original, mode, code, tmp_path
):
    """Forced group stops fail; live parents or groups retain their input FIFO."""
    script = f"""set -euo pipefail
stop_vm() {{ :; }}
secure_stop() {{ :; }}
wait_for_exit() {{ return {7 if mode == 'parent-live' else 0}; }}
rm() {{ echo remove; }}
group_gone={1 if mode in ['before', 'parent-live'] else 0}
kill() {{
 if [[ $1 == -0 && ${{2:-}} == -- ]]; then return "$group_gone"; fi
 if [[ $1 == -0 ]]; then return {0 if mode == 'parent-live' else 1}; fi
 echo signal-${{1#-}}
 if [[ $1 == -TERM ]]; then
  if [[ {mode} == natural || {mode} == term-race ]]; then group_gone=1; fi
  return {11 if mode == 'term-error' else 7 if mode == 'term-race' else 0}
 fi
 if [[ {mode} != forced-live && {mode} != kill-error ]]; then group_gone=1; fi
 return {7 if mode in ['kill-race', 'kill-error'] else 0}
}}
proxy_child=47
exec 3>/dev/null
{windows_cleanup['cleanup_failed']}
{windows_cleanup['cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (original or code), result.stderr
    calls = []
    if mode not in ["before", "parent-live"]:
        calls.append(b"signal-TERM")
    if mode in ["forced-gone", "forced-live", "kill-race", "term-error", "kill-error"]:
        calls.append(b"signal-KILL")
    if mode not in ["forced-live", "kill-error", "parent-live"]:
        calls.append(b"remove")
    assert result.stdout.splitlines() == calls
    assert (b"CONNECT cleanup failed:" in result.stderr) == (code != 0)


@pytest.mark.parametrize("original", [0, 42, 130, 143])
@pytest.mark.parametrize(
    "failure,code",
    [
        ("none", 0),
        ("stop", 7),
        ("secure", 11),
        ("term", 19),
        ("kill", 23),
        ("wait", 29),
        ("all", 7),
    ],
)
def test_tap_cleanup_preserves_error_and_attempts_all(
    windows_cleanup, original, failure, code, tmp_path
):
    """Every owned cleanup step runs without replacing the primary failure."""
    values = {
        key: value if failure in [key, "all"] else 0
        for key, value in {
            "stop": 7,
            "secure": 11,
            "term": 19,
            "kill": 23,
            "wait": 29,
        }.items()
    }
    forced = failure in ["kill", "all"]
    script = f"""set -euo pipefail
stop() {{ echo stop; return {values['stop']}; }}
secure_stop() {{ echo secure; return {values['secure']}; }}
kill() {{
 if [[ $1 == -0 ]]; then return 0; fi
 echo echo-${{1#-}}
 if [[ $1 == -TERM ]]; then return {values['term']}; fi
 return {values['kill']}
}}
polls=0
secure_wait_stopped() {{
 polls=$((polls+1))
 if [[ $polls == 1 ]]; then return {1 if forced else 0}; fi
 return 0
}}
wait() {{ echo echo-wait; return {values['wait']}; }}
echo_child=47
secure_pids=()
secure_current=state
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (original or code), result.stderr
    expected = [b"stop", b"secure"]
    expected += [b"echo-TERM"]
    if forced:
        expected += [b"echo-KILL"]
    assert result.stdout.splitlines() == [*expected, b"echo-wait"]
    assert (b"TAP cleanup failed:" in result.stderr) == (failure != "none")


@pytest.mark.parametrize(
    "term,wait,sockets,expected",
    [
        (0, 0, 0, 0),
        (7, 0, 0, 7),
        (0, 143, 0, 0),
        (0, 137, 0, 137),
        (0, 0, 11, 11),
        (7, 137, 11, 7),
    ],
)
def test_tap_stop_keeps_first_failure(
    windows_cleanup, term, wait, sockets, expected, tmp_path
):
    """Signal the root VMM, wait for its wrapper and attempt socket cleanup."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
gone=0
kill() {{ return "$gone"; }}
secure_wait_stopped() {{ gone=1; return 0; }}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == kill ]]; then echo "$2-$3"; return {term}; fi
 echo sockets; return {sockets}
}}
wait() {{ echo wait-$1; return {wait}; }}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
echo child="$child"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == expected
    assert result.stdout.splitlines() == [
        b"-TERM-61",
        b"wait-41",
        b"sockets",
        b"child=",
    ]


@pytest.mark.parametrize(
    "kill_result,still_live", [(0, False), (7, False), (7, True), (0, True)]
)
def test_tap_stop_bounds_forced_kill(
    windows_cleanup, kill_result, still_live, tmp_path
):
    """Do not wait for a live wrapper; retain ownership and strict forced-stop failure."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
forced=0
kill() {{ if [[ $forced == 0 ]]; then return 0; fi; return {0 if still_live else 1}; }}
sleep() {{ :; }}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 echo "$2-$3"
 if [[ $2 == -KILL ]]; then forced=1; return {kill_result}; fi
 return 0
}}
wait() {{ echo wait-$1; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
echo child="$child"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (kill_result if still_live and kill_result else 1)
    expected = [b"-TERM-61", b"-KILL-61"]
    if not still_live:
        expected += [b"wait-41", b"sockets"]
    assert result.stdout.splitlines() == [
        *expected,
        b"child=41" if still_live else b"child=",
    ]


@pytest.mark.parametrize(
    "pid,read_error", [("61", 7), ("-1", 0), ("0", 0), ("61 62", 0), ("", 0)]
)
def test_tap_stop_rejects_invalid_root_pid(windows_cleanup, pid, read_error, tmp_path):
    """A failed PID read or invalid scalar must never become a root signal target."""
    script = f"""set -euo pipefail
child=41
run=fixture
base=fixture
kill() {{ return 0; }}
sleep() {{ :; }}
sudo() {{
 if [[ $1 == cat ]]; then echo '{pid}'; return {read_error}; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 echo unexpected-signal; return 0
}}
wait() {{ echo unexpected-wait; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_stop']}
result=0
stop || result=$?
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (read_error or 1)
    assert result.stdout.splitlines() == []


@pytest.mark.parametrize(
    "term_error,kill_error,still_live",
    [(0, 0, False), (0, 7, False), (0, 7, True), (0, 0, True), (11, 7, True)],
)
def test_tap_echo_stop_does_not_wait_for_live_child(
    windows_cleanup, term_error, kill_error, still_live, tmp_path
):
    """Echo cleanup is bounded even after both signals fail and preserves its PID."""
    script = f"""set -euo pipefail
stop() {{ :; }}
secure_stop() {{ :; }}
echo_child=47
forced=0
kill() {{
 if [[ $1 == -TERM ]]; then echo term; return {term_error}; fi
 if [[ $1 == -KILL ]]; then forced=1; echo kill; return {kill_error}; fi
 if [[ $forced == 1 ]]; then return {0 if still_live else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo wait; return 0; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
exit() {{ echo child="$echo_child"; builtin exit "$1"; }}
trap cleanup EXIT
exit 0
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (term_error or (kill_error if still_live else 0) or 1)
    expected = [b"child=47", b"term", b"kill"]
    if not still_live:
        expected += [b"wait"]
    assert result.stdout.splitlines() == [
        *expected,
        b"child=47" if still_live else b"child=",
    ]


@pytest.mark.parametrize("controller", ["tap", "echo"])
def test_tap_stop_allows_natural_exit_during_term(
    windows_cleanup, controller, tmp_path
):
    """An observed-gone signal race is benign for both the VMM and echo child."""
    script = f"""set -euo pipefail
child=41
echo_child=47
run=fixture
base=fixture
gone=0
kill() {{
 if [[ $1 == -0 ]]; then return "$gone"; fi
 echo term; gone=1; return 7
}}
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == rm ]]; then echo sockets; return 0; fi
 kill "$2" "$3"
}}
wait() {{ echo wait-$1; return 143; }}
{windows_cleanup['secure_wait_stopped']}
"""
    if controller == "tap":
        script += f"""{windows_cleanup['tap_stop']}
result=0
stop || result=$?
exit "$result"
"""
    else:
        script += f"""stop() {{ :; }}
secure_stop() {{ :; }}
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit 0
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == (
        [b"term", b"wait-41", b"sockets"]
        if controller == "tap"
        else [b"term", b"wait-47"]
    )


@pytest.mark.parametrize("original", [0, 42, 130, 143])
def test_tap_cleanup_survives_failed_diagnostics(windows_cleanup, original, tmp_path):
    """A failed stderr write must not abort EXIT cleanup or replace its primary exit."""
    script = f"""set -euo pipefail
stop() {{ echo stop; return 7; }}
secure_stop() {{ echo secure; return 11; }}
secure_wait_stopped() {{ return 0; }}
kill() {{ if [[ $1 == -0 ]]; then return 0; fi; echo echo-TERM; return 19; }}
wait() {{ echo echo-wait; return 23; }}
printf() {{ return 31; }}
echo_child=47
{windows_cleanup['tap_cleanup_failed']}
{windows_cleanup['tap_cleanup']}
trap cleanup EXIT
exit {original}
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (original or 7)
    assert result.stdout.splitlines() == [
        b"stop",
        b"secure",
        b"echo-TERM",
        b"echo-wait",
    ]


@pytest.mark.parametrize("operation", ["stop", "vmm"])
def test_windows_cleanup_survives_failed_diagnostics(
    windows_cleanup, operation, tmp_path
):
    """A helper failure keeps its status and remaining cleanup even without stderr."""
    script = f"""set -euo pipefail
secure_pids=(51 52)
secure_current=state
child=41
run=fixture
base=fixture
sudo() {{
 if [[ $1 == cat ]]; then echo 61; return 0; fi
 if [[ $1 == kill ]]; then kill "$2" "$3"; return $?; fi
 echo unexpected-socket-removal
}}
kill() {{
 if [[ $1 == -0 ]]; then return 0; fi
 echo "$1-$2"
 if [[ $1 == -TERM ]]; then return 7; fi
 return 11
}}
sleep() {{ :; }}
printf() {{ return 31; }}
wait() {{ echo unexpected-wait; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['secure_stop']}
{windows_cleanup['tap_stop']}
trap 'code=$?; echo "result=$code remaining=${{secure_pids[*]}}"' EXIT
{'stop' if operation == 'vmm' else 'secure_' + operation}
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    if operation == "stop":
        assert result.returncode == 7
        assert result.stdout.splitlines() == [
            b"-TERM-51",
            b"-TERM-52",
            b"-KILL-51",
            b"-KILL-52",
            b"result=7 remaining=51 52",
        ]
    else:
        assert result.returncode == 7
        assert result.stdout.splitlines() == [
            b"-TERM-61",
            b"-KILL-61",
            b"result=7 remaining=51 52",
        ]


@pytest.mark.parametrize("kill_result", [0, 7])
@pytest.mark.parametrize(
    "term,still_live", [(0, False), (0, True), (11, False), (11, True)]
)
def test_connect_stop_retains_live_ownership(
    windows_cleanup, term, still_live, kill_result, tmp_path
):
    """A failed actual forced kill must retain the live child and its sockets."""
    script = f"""set -euo pipefail
child=41
forced=0
kill() {{
 if [[ $1 == -TERM ]]; then echo TERM-$2; return {term}; fi
 if [[ $1 == -KILL ]]; then echo KILL-$2; forced=1; return {kill_result}; fi
 if [[ $forced == 1 ]]; then return {0 if still_live else 1}; fi
 return 0
}}
sleep() {{ :; }}
wait() {{ echo WAIT-$1; return 0; }}
remove_sockets() {{ echo SOCKETS; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
echo "child=$child"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (term or ((kill_result or 1) if still_live else 0))
    expected = [b"TERM-41", b"KILL-41"]
    if still_live:
        expected += [b"child=41"]
    else:
        expected += [b"WAIT-41", b"SOCKETS", b"child="]
    assert result.stdout.splitlines() == expected


@pytest.mark.parametrize("wait_status,expected", [(0, 1), (143, 1), (7, 7)])
def test_connect_stop_checks_liveness_after_wait(
    windows_cleanup, wait_status, expected, tmp_path
):
    """A wait result alone cannot authorise forgetting an observed live child."""
    script = f"""set -euo pipefail
child=41
kill() {{ return 0; }}
wait_for_exit() {{ return {wait_status}; }}
remove_sockets() {{ echo unexpected-sockets; }}
{windows_cleanup['stop_vm']}
result=0
stop_vm || result=$?
echo "child=$child"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == expected
    assert result.stdout.splitlines() == [b"child=41"]


@pytest.mark.parametrize(
    "mode,kill_status,wait_status,expected,sleeps",
    [
        ("natural", 0, 143, 143, 0),
        ("killed", 0, 137, 137, 100),
        ("failed-gone", 7, 0, 0, 100),
        ("live", 0, 0, 1, 200),
        ("failed-live", 7, 0, 7, 100),
        ("late-exit", 0, 137, 137, 103),
    ],
)
def test_connect_wait_observes_bounded_disappearance(
    windows_cleanup, mode, kill_status, wait_status, expected, sleeps, tmp_path
):
    """Successful KILL is a request; only observed exit permits the blocking wait."""
    script = f"""set -euo pipefail
forced=0
post_probes=0
sleeps=0
kill() {{
 if [[ $1 == -KILL ]]; then forced=1; echo KILL-$2; return {kill_status}; fi
 if [[ $forced == 0 ]]; then return {1 if mode == 'natural' else 0}; fi
 post_probes=$((post_probes+1))
 {'if [[ $post_probes -le 3 ]]; then return 0; fi' if mode == 'late-exit' else ':'}
 return {0 if mode in ['live', 'failed-live'] else 1}
}}
sleep() {{ sleeps=$((sleeps+1)); }}
wait() {{
 if kill -0 "$1"; then echo unexpected-live-wait; return 99; fi
 echo WAIT-$1; return {wait_status}
}}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['wait_for_exit']}
result=0
wait_for_exit 41 || result=$?
echo "sleeps=$sleeps"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == expected
    calls = [] if mode == "natural" else [b"KILL-41"]
    if mode not in ["live", "failed-live"]:
        calls += [b"WAIT-41"]
    assert result.stdout.splitlines() == [*calls, f"sleeps={sleeps}".encode()]
