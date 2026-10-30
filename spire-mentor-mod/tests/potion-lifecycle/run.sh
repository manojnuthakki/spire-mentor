#!/usr/bin/env bash
# Builds and runs the potion action lifecycle test (PotionLifecycle.csproj says against what).
set -eo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
pycli="$here/../../../sts2-pycli"
sts2="${STS2_DLL:-$pycli/lib/sts2.dll}"
godot="${GODOT_DLL:-$pycli/src/GodotStubs/bin/Debug/net9.0/GodotSharp.dll}"
dotnet="${DOTNET:-$HOME/.dotnet/dotnet}"
out="${TMPDIR:-/tmp}/potion-lifecycle"
for f in "$sts2" "$godot"; do
  [ -f "$f" ] || { echo "missing $f (build the Headless host first, or set STS2_DLL / GODOT_DLL)" >&2; exit 2; }
done
"$dotnet" build "$here/PotionLifecycle.csproj" -c Release -o "$out" -v quiet -nologo \
  -p:Sts2Dll="$sts2" -p:GodotDll="$godot" -p:BaseIntermediateOutputPath="$out/obj/" | grep -E "error|Build succeeded" || true
"$dotnet" "$out/PotionLifecycle.dll"
