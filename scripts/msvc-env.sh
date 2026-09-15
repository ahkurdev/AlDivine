#!/usr/bin/env bash
# MSVC build environment for the AlDivine Rust workspace (Windows + git-bash).
# Source it: `source scripts/msvc-env.sh` (or run via the wrapper below).
set -e
VS="C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools"
SDK="C:/Program Files (x86)/Windows Kits/10"
SDKVER=10.0.22621.0
VC="$VS/VC/Tools/MSVC/14.44.35207"
export PATH="$VC/bin/Hostx64/x64:$SDK/bin/$SDKVER/x64:$PATH"
export INCLUDE="$VC/include;$SDK/Include/$SDKVER/ucrt;$SDK/Include/$SDKVER/um;$SDK/Include/$SDKVER/shared"
export LIB="$VC/lib/x64;$SDK/Lib/$SDKVER/ucrt/x64;$SDK/Lib/$SDKVER/um/x64"
set +e
