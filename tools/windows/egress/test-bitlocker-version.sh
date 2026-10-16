#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
fixture=${1:?Set a new absolute workspace fixture directory}
[[ "$fixture" = /* ]]
mkdir "$fixture"
trap 'rm -r "$fixture"' EXIT
version_check=$(sed -n '/^"\$bdemount" -V |/p' "$(dirname "$0")/bitlocker-verify.sh")
test -n "$version_check"
cat > "$fixture/bdemount" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
test "$1" = -V
printf 'bdemount %s\n' "$VERSION"
sleep 0.02
printf '%262144s\n' ''
exit "$PRODUCER_EXIT"
SCRIPT
chmod +x "$fixture/bdemount"
for version in 20260916 20260101; do
    result=0
    VERSION=$version PRODUCER_EXIT=0 bdemount=$fixture/bdemount bash -o pipefail -c "$version_check" || result=$?
    expected=0
    if [[ "$version" != 20260916 ]]; then expected=1; fi
    test "$result" = "$expected"
    echo "$version: expected exit $expected after all version output"
done
result=0
VERSION=20260916 PRODUCER_EXIT=17 bdemount=$fixture/bdemount bash -o pipefail -c "$version_check" || result=$?
test "$result" = 17
echo 'producer failure: expected exit 17'
