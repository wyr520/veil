#!/usr/bin/env bash
# Run the share-link parser tests on a plain JVM.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SDK="${ANDROID_SDK_ROOT:-$HOME/AndroidBuild/sdk}"
JDK="${JAVA_HOME_DIR:-$HOME/AndroidBuild/jdk17}"
AJ="$SDK/platforms/android-34/android.jar"

rm -rf "$HERE/build/jvm"
mkdir -p "$HERE/build/jvm"
"$JDK/bin/javac.exe" --release 11 -encoding UTF-8 -nowarn -cp "$AJ" \
  -d "$HERE/build/jvm" \
  "$HERE/src/com/veil/app/Node.java" \
  "$HERE/src/com/veil/app/B64.java" \
  "$HERE/tools/jvmtest/ParseTest.java"
"$JDK/bin/java.exe" -Dfile.encoding=UTF-8 \
  -cp "$(cygpath -w "$HERE/build/jvm");$(cygpath -w "$AJ")" ParseTest
