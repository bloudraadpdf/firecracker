# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check protected-service cleanup ownership without a guest."""

from subprocess import run

import pytest


@pytest.mark.parametrize(
    "mode",
    [
        "natural",
        "term-gone",
        "term-live",
        "kill-gone",
        "kill-failed-gone",
        "kill-live",
        "wait",
    ],
)
def test_secure_stop_retains_live_ownership(windows_cleanup, mode, tmp_path):
    """Attempt both helpers and retain ownership of any live PID."""
    forced = mode in ["term-live", "kill-gone", "kill-failed-gone", "kill-live"]
    script = f"""set -euo pipefail
secure_pids=(51 52)
secure_current=state
forced51=0
forced52=0
kill() {{
 local name=forced${{2}}
 if [[ $1 == -TERM ]]; then
  echo term-$2
  return {7 if mode.startswith('term-') else 0}
 fi
 if [[ $1 == -KILL ]]; then
  echo kill-$2
  printf -v "$name" %s 1
  return {11 if mode in ['kill-failed-gone', 'kill-live'] else 0}
 fi
 if [[ ${{!name}} == 1 ]]; then return {0 if mode == 'kill-live' else 1}; fi
 return {0 if forced else 1}
}}
sleep() {{ :; }}
wait() {{ echo wait-$1; return {17 if mode == 'wait' else 0}; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['secure_stop']}
result=0
secure_stop || result=$?
echo remaining="${{secure_pids[*]}}"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    expected_result = {
        "term-live": 7,
        "kill-gone": 1,
        "kill-failed-gone": 1,
        "kill-live": 11,
        "wait": 17,
    }.get(mode, 0)
    assert result.returncode == expected_result, result.stderr
    expected = [b"term-51", b"term-52"]
    for pid in [51, 52]:
        if forced:
            expected += [f"kill-{pid}".encode()]
        if mode != "kill-live":
            expected += [f"wait-{pid}".encode()]
    expected += [b"remaining=51 52"] if mode == "kill-live" else [b"remaining="]
    assert result.stdout.splitlines() == expected


@pytest.mark.parametrize("live", [False, True])
def test_secure_wait_is_bounded(windows_cleanup, live, tmp_path):
    """The shared poll has a fixed bound and never calls the blocking wait builtin."""
    script = f"""set -euo pipefail
probes=0
sleeps=0
kill() {{ probes=$((probes+1)); return {0 if live else 1}; }}
sleep() {{ sleeps=$((sleeps+1)); }}
wait() {{ echo unexpected-wait; }}
{windows_cleanup['secure_wait_stopped']}
result=0
secure_wait_stopped 51 || result=$?
echo "$probes $sleeps"
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == int(live)
    assert result.stdout.splitlines() == ([b"101 100"] if live else [b"1 0"])


@pytest.mark.parametrize("controller", ["tap", "connect"])
@pytest.mark.parametrize("original", [0, 42, 130, 143])
def test_controller_cleanup_retains_advanced_state(
    windows_cleanup, controller, original, tmp_path
):
    """Stopping a controller must retain helper state paired with disk writes."""
    disk = tmp_path / "windows.raw"
    disk.write_bytes(b"disk changed during failed cold boot")
    state = tmp_path / "state"
    state.joinpath("tpm").mkdir(parents=True)
    expected = {
        "store.json": b"advanced variable store",
        "store.json.pending": b"pending variable update",
        "store.lock": b"",
        "tpm/tpm2-00.permall": b"advanced TPM state",
        "tpm/.lock": b"",
    }
    for name, raw in expected.items():
        state.joinpath(name).write_bytes(raw)
    script = f"""set -euo pipefail
{windows_cleanup['secure_state']}
secure_current=state
stop() {{ :; }}
stop_vm() {{ :; }}
wait_for_exit() {{ :; }}
kill() {{ return 1; }}
echo_child=
proxy_child=47
{windows_cleanup['tap_cleanup_failed'] if controller == 'tap' else windows_cleanup['cleanup_failed']}
{windows_cleanup['tap_cleanup'] if controller == 'tap' else windows_cleanup['cleanup']}
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
    assert result.returncode == original, result.stderr
    assert disk.read_bytes() == b"disk changed during failed cold boot"
    assert sorted(
        str(path.relative_to(state)) for path in state.rglob("*") if path.is_file()
    ) == sorted(expected)
    for name, raw in expected.items():
        assert state.joinpath(name).read_bytes() == raw
