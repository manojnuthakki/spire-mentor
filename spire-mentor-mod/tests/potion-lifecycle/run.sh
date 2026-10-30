#!/usr/bin/env bash
# Builds and runs the potion action lifecycle test (PotionLifecycle.csproj says against what).
# Exits non-zero when the build fails or a case fails; each run builds into a fresh directory, so
# an older binary can never stand in for a failed build. --check-failed-build checks just that.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
pycli="$here/../../../sts2-pycli"
sts2="${STS2_DLL:-$pycli/lib/sts2.dll}"
godot="${GODOT_DLL:-$pycli/src/GodotStubs/bin/Debug/net9.0/GodotSharp.dll}"
dotnet="${DOTNET:-$HOME/.dotnet/dotnet}"
base="${TMPDIR:-$HOME/.cache/sts2_agent/tmp}"
for f in "$sts2" "$godot"; do
  [ -f "$f" ] || { echo "missing $f (build the Headless host first, or set STS2_DLL / GODOT_DLL)" >&2; exit 2; }
done

build() {  # build <out> <sts2.dll>: the build's own exit status, its errors shown
  local log="$1/build.log" status=0
  "$dotnet" build "$here/PotionLifecycle.csproj" -c Release -o "$1" -v quiet -nologo \
    -p:Sts2Dll="$2" -p:GodotDll="$godot" -p:BaseIntermediateOutputPath="$1/obj/" >"$log" 2>&1 || status=$?
  grep -E "error|Build succeeded" "$log" | sort -u || true
  return "$status"
}

out="$(mktemp -d "$base/potion-lifecycle.XXXXXX")"
trap 'rm -rf "$out"' EXIT
if [ "${1:-}" = "--check-failed-build" ]; then
  # A build that can't compile (sts2.dll replaced by an assembly without the game's types) must fail.
  if build "$out" "$godot" >/dev/null; then echo "FAIL: a broken build passed" >&2; exit 1; fi
  echo "ok   a failed build fails"
  exit 0
fi
build "$out" "$sts2" || { echo "build failed" >&2; exit 1; }
"$dotnet" "$out/PotionLifecycle.dll"
