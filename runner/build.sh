#!/usr/bin/env bash
# Freeze the VirtuAI CLI (virtuai-cli, from PyPI) into a self-contained
# directory for VirtuAI.app: Python, its packages and the `virtuai` executable,
# so the person installing the app needs no Python, pip or terminal.
#
#   runner/build.sh            -> runner/dist/virtuai/virtuai
#
# The CLI version is pinned in runner/cli-version: each app release ships the
# runner it was tested with. The app needs a CLI with `virtuai run --desktop`.
#
# --onedir, not --onefile: a onefile build unpacks itself to a temp directory on
# every start, which is slow and which Gatekeeper treats as unsigned code.
#
# Builds for the machine's architecture (arm64 on Apple silicon). With
# APPLE_SIGNING_IDENTITY set, signs every binary inside with the hardened
# runtime, as notarization requires; Tauri signs the app around it but does not
# sign Mach-O files it finds among the resources.
#
# Environment:
#   VIRTUAI_RUNNER_PYTHON   Python version to freeze (default 3.12)
#   VIRTUAI_RUNNER_EXTRAS   cli extras to include (default "browser"; "" for none)
#   VIRTUAI_CLI_SOURCE      a local checkout of the CLI to freeze instead of the
#                           pinned PyPI release (to try a CLI change before
#                           publishing it)
#   APPLE_SIGNING_IDENTITY  "Developer ID Application: … (TEAMID)" to sign
set -euo pipefail
trap 'echo "build.sh: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="$HERE/build"
DIST="$HERE/dist"
PYTHON="${VIRTUAI_RUNNER_PYTHON:-3.12}"
EXTRAS="${VIRTUAI_RUNNER_EXTRAS-browser}"

command -v uv >/dev/null || { echo "uv is required: https://docs.astral.sh/uv/" >&2; exit 1; }

rm -rf "$BUILD" "$DIST"
# uv's own Python, never one installed on the machine: GitHub's macOS image has
# python.org's framework build, which PyInstaller copies as a Python.framework
# of symlinks. Tauri flattens those when it copies the resources, breaking the
# framework's signature, and notarization rejects the app. uv's standalone
# build ships a plain libpython dylib.
uv venv -q --python-preference only-managed --python "$PYTHON" "$BUILD/venv"
if [ -n "${VIRTUAI_CLI_SOURCE:-}" ]; then
  SOURCE="$(cd "$VIRTUAI_CLI_SOURCE" && pwd)"
  PKG="$SOURCE"; [ -n "$EXTRAS" ] && PKG="$SOURCE[$EXTRAS]"
else
  VERSION="$(tr -d '[:space:]' < "$HERE/cli-version")"
  PKG="virtuai-cli==$VERSION"; [ -n "$EXTRAS" ] && PKG="virtuai-cli[$EXTRAS]==$VERSION"
fi
echo "build.sh: freezing $PKG" >&2
# --refresh-package: the CI restores uv's cache from an earlier run, whose copy
# of PyPI's index for virtuai-cli predates a version published minutes ago —
# 0.2.8 and 0.2.9 each failed once with "no version of virtuai-cli==…".
VIRTUAL_ENV="$BUILD/venv" uv pip install -q --refresh-package virtuai-cli "$PKG" "pyinstaller==6.22.3"

args=(
  --noconfirm --clean --onedir --console --name virtuai
  --distpath "$DIST" --workpath "$BUILD/work" --specpath "$BUILD"
  # Imported inside functions, and some only by name.
  --collect-submodules virtuai_cli
  # Its backends are found through entry points, which need the metadata.
  --collect-submodules keyring --copy-metadata keyring
  # Textual's CSS and llmfit's version() read package files and metadata.
  --collect-data textual --copy-metadata llmfit
)
case ",$EXTRAS," in
  *,browser,*) args+=(--collect-all browser_use) ;;
esac

"$BUILD/venv/bin/pyinstaller" "${args[@]}" "$HERE/entry.py"

# llmfit is a native binary the wheel installs as a script. The CLI looks for
# it beside sys.executable (ollama_state._llmfit), which frozen is this dir.
cp "$BUILD/venv/bin/llmfit" "$DIST/virtuai/llmfit"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  # Inside out: libraries first, the executables that load them last.
  find "$DIST/virtuai" -type f \( -name '*.so' -o -name '*.dylib' \) -print0 |
    xargs -0 codesign --force --timestamp --options runtime \
      --sign "$APPLE_SIGNING_IDENTITY"
  find "$DIST/virtuai/_internal" -type f -name Python -print0 |
    xargs -0 -r codesign --force --timestamp --options runtime \
      --sign "$APPLE_SIGNING_IDENTITY"
  for exe in "$DIST/virtuai/llmfit" "$DIST/virtuai/virtuai"; do
    codesign --force --timestamp --options runtime \
      --entitlements "$HERE/entitlements.plist" \
      --sign "$APPLE_SIGNING_IDENTITY" "$exe"
  done
fi

"$HERE/smoke.sh" "$DIST/virtuai/virtuai"
du -sh "$DIST/virtuai"
