# Invoked by the Nix-packaged wrapper, outside any derivation.
if [[ $# != 5 ]]; then
  echo 'Usage: agentix-android-sign INPUT.apk OUTPUT.apk KEYSTORE ALIAS PASSWORD_FILE' >&2
  exit 2
fi
umask 077
input=$(realpath -e -- "$1")
output=$(realpath -m -- "$2")
keystore=$(realpath -e -- "$3")
alias=$4
password_file=$(realpath -e -- "$5")
for private_path in "$keystore" "$password_file" "$output"; do
  case "$private_path" in
    /nix/store|/nix/store/*)
      echo 'Signing secrets and output must be outside the Nix store.' >&2
      exit 2
      ;;
  esac
done
if [[ -e "$output" || -L "$output" ]]; then
  echo 'Output already exists; choose a new output path.' >&2
  exit 2
fi
work=$(mktemp -d -- "$(dirname -- "$output")/.agentix-sign.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
"$AGENTIX_ANDROID_BUILD_TOOLS/zipalign" -f -p 4 "$input" "$work/aligned.apk"
"$AGENTIX_ANDROID_BUILD_TOOLS/apksigner" sign \
  --ks "$keystore" --ks-key-alias "$alias" \
  --ks-pass "file:$password_file" --v4-signing-enabled false \
  --out "$work/signed.apk" "$work/aligned.apk"
"$AGENTIX_ANDROID_BUILD_TOOLS/apksigner" verify "$work/signed.apk"
# A hard link publishes only a verified APK and fails if output appeared meanwhile.
ln -- "$work/signed.apk" "$output"
echo "Signed and verified: $output"
