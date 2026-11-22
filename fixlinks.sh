#!/bin/bash
# 修复被 AV 隔离的 build-script-build.exe 硬链接
cd /f/mc-native-exp
fixed=0
for d in target/release/build/*/; do
  hashed=$(ls "$d" 2>/dev/null | grep -E "^build-script-build-.*\.exe$" | head -1)
  if [ -n "$hashed" ] && [ ! -f "${d}build-script-build.exe" ]; then
    ln "${d}${hashed}" "${d}build-script-build.exe" 2>/dev/null && { fixed=$((fixed+1)); echo "link: ${d#target/release/build/}"; }
  fi
done
echo "fixed=$fixed"
