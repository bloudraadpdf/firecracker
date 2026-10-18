# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
"""Check protected-service cleanup ownership without a guest."""

from subprocess import run

import pytest


@pytest.mark.parametrize("files,tree", [(0, 0), (7, 0), (0, 11), (7, 11)])
def test_secure_discard_keeps_first_failure(windows_cleanup, files, tree, tmp_path):
    """Both actual discard command positions run and the first failure survives."""
    script = f"""set -euo pipefail
rm() {{
 if [[ $1 == -f ]]; then echo files; return {files}; fi
 echo tree; return {tree}
}}
secure_current=state
secure_pids=()
{windows_cleanup['secure_discard']}
result=0
secure_discard || result=$?
exit "$result"
"""
    result = run(
        ["bash", "-c", script],
        cwd=tmp_path,
        capture_output=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == (files or tree)
    assert result.stdout.splitlines() == [b"files", b"tree"]


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
    """Attempt both helpers, retain live PIDs and refuse to discard their inputs."""
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
rm() {{ echo discard; }}
{windows_cleanup['secure_wait_stopped']}
{windows_cleanup['secure_stop']}
{windows_cleanup['secure_discard']}
result=0
secure_stop || result=$?
echo remaining="${{secure_pids[*]}}"
discard=0
secure_discard || discard=$?
echo discard-result="$discard"
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
    if mode != "kill-live":
        expected += [b"discard", b"discard"]
    expected += [b"discard-result=1" if mode == "kill-live" else b"discard-result=0"]
    assert result.stdout.splitlines() == expected
    assert (b"Refusing to discard live" in result.stderr) == (mode == "kill-live")


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


@pytest.mark.parametrize("live", [False, True])
def test_secure_discard_checks_retained_helper(windows_cleanup, live, tmp_path):
    """Only observed live helpers prevent discard; a later natural exit is safe."""
    script = f"""set -euo pipefail
secure_pids=(51)
secure_current=state
kill() {{ return {0 if live else 1}; }}
rm() {{ echo discard; }}
{windows_cleanup['secure_discard']}
result=0
secure_discard || result=$?
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
    assert result.stdout.splitlines() == ([] if live else [b"discard", b"discard"])
