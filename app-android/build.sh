#!/usr/bin/env bash
# Build veil.apk with nothing but the Android SDK build-tools.
#
# No Gradle, no AndroidX, no network: aapt2 -> javac -> d8 -> zipalign -> apksigner.
# That keeps the build reproducible on a machine that only has the SDK unpacked.
#
#   ./build.sh            # debug build, signed with the bundled debug key
#   ./build.sh release    # same pipeline, but expects release.jks + env vars
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SDK="${ANDROID_SDK_ROOT:-$HOME/AndroidBuild/sdk}"
JDK="${JAVA_HOME_DIR:-$HOME/AndroidBuild/jdk17}"
BT="$SDK/build-tools/34.0.0"
PLATFORM="$SDK/platforms/android-34/android.jar"
OUT="$HERE/build"
MODE="${1:-debug}"

for f in "$BT/aapt2.exe" "$BT/zipalign.exe" "$BT/apksigner.bat" "$PLATFORM"; do
  [[ -e "$f" ]] || { echo "missing: $f" >&2; exit 1; }
done

export JAVA_HOME="$JDK"
export PATH="$JDK/bin:$BT:$PATH"

echo "==> icon"
python3 "$HERE/tools/icon.py" >/dev/null 2>&1 || python "$HERE/tools/icon.py"

echo "==> clean"
rm -rf "$OUT"
mkdir -p "$OUT/gen" "$OUT/classes" "$OUT/dex"

echo "==> aapt2 compile"
"$BT/aapt2.exe" compile --dir "$HERE/res" -o "$OUT/res.zip"

echo "==> aapt2 link"
"$BT/aapt2.exe" link \
  -o "$OUT/unsigned.apk" \
  -I "$PLATFORM" \
  --manifest "$HERE/AndroidManifest.xml" \
  -R "$OUT/res.zip" \
  --java "$OUT/gen" \
  --min-sdk-version 21 \
  --target-sdk-version 34 \
  --auto-add-overlay

echo "==> javac"
find "$HERE/src" "$OUT/gen" -name '*.java' | while read -r f; do cygpath -w "$f"; done > "$OUT/sources.txt"
"$JDK/bin/javac.exe" --release 11 -encoding UTF-8 -nowarn \
  -cp "$PLATFORM" \
  -d "$OUT/classes" \
  @"$OUT/sources.txt"

echo "==> d8"
find "$OUT/classes" -name '*.class' | while read -r f; do cygpath -w "$f"; done > "$OUT/classes.txt"
"$BT/d8.bat" --lib "$PLATFORM" --min-api 21 --output "$OUT/dex" @"$OUT/classes.txt" >/dev/null

echo "==> package"
python - "$OUT" <<'PY'
import os, sys, zipfile
out = sys.argv[1]
apk = os.path.join(out, "unsigned.apk")
with zipfile.ZipFile(apk, "a", zipfile.ZIP_DEFLATED) as z:
    z.write(os.path.join(out, "dex", "classes.dex"), "classes.dex")
print("  classes.dex injected")
PY

echo "==> zipalign"
"$BT/zipalign.exe" -f -p 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"

KEYSTORE="$HERE/$MODE.jks"
if [[ ! -f "$KEYSTORE" ]]; then
  echo "==> generating $MODE keystore"
  PASS="${VEIL_STORE_PASS:-android}"
  "$JDK/bin/keytool.exe" -genkeypair -v \
    -keystore "$KEYSTORE" -alias veil -keyalg RSA -keysize 2048 -validity 10950 \
    -storepass "$PASS" -keypass "$PASS" \
    -dname "CN=Veil, OU=proxy, O=veil, L=-, S=-, C=CN" >/dev/null 2>&1
fi

PASS="${VEIL_STORE_PASS:-android}"
echo "==> sign"
"$BT/apksigner.bat" sign \
  --ks "$KEYSTORE" --ks-key-alias veil \
  --ks-pass "pass:$PASS" --key-pass "pass:$PASS" \
  --min-sdk-version 21 \
  --out "$HERE/veil-$MODE.apk" "$OUT/aligned.apk"

"$BT/apksigner.bat" verify --print-certs "$HERE/veil-$MODE.apk" | head -3
echo
echo "built: $HERE/veil-$MODE.apk  ($(stat -c%s "$HERE/veil-$MODE.apk" 2>/dev/null || wc -c < "$HERE/veil-$MODE.apk") bytes)"
