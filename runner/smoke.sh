#!/usr/bin/env bash
# What the app relies on from a frozen runner, checked on the binary itself:
# it starts, it has the --desktop flag, and unpaired it says so in the
# protocol instead of exiting silently. Runs with an empty HOME so it never
# sees a real pairing.
set -euo pipefail
trap 'echo "smoke: failed at line $LINENO: $BASH_COMMAND" >&2' ERR
CLI="${1:?usage: smoke.sh path/to/virtuai}"
TMP_HOME="$(mktemp -d)"
trap 'rm -rf "$TMP_HOME"' EXIT

"$CLI" --help >/dev/null
# Captured, not piped into `grep -q`: grep stops reading at the first match,
# the CLI dies of SIGPIPE writing the rest, and pipefail turns that into a
# silent failure — which is what happened on macOS, whose pipe buffer is small.
help="$(COLUMNS=200 "$CLI" run --help)"
grep -q -- "--desktop" <<<"$help" || { echo "smoke: 'run --help' has no --desktop" >&2; exit 1; }

# Unpaired, the runner exits 1 on purpose; `|| code=$?` keeps that from
# tripping set -e and the ERR trap.
code=0
out="$(HOME="$TMP_HOME" PYTHON_KEYRING_BACKEND=keyring.backends.fail.Keyring \
  "$CLI" run --desktop --allow-multiple </dev/null 2>&1)" || code=$?
if [ "$code" -ne 1 ] || ! grep -q '^@@virtuai {"type": "state", "state": "not_paired"}' <<<"$out"; then
  echo "smoke: unexpected result from an unpaired runner (exit $code):" >&2
  echo "$out" >&2
  exit 1
fi
echo "smoke: ok"
