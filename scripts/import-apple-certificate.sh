#!/usr/bin/env bash
# Used only on ephemeral GitHub macOS runners. Keep private files off the repo.
set -euo pipefail
umask 077

: "${RUNNER_TEMP:?}" "${APPLE_CERTIFICATE:?}" "${APPLE_CERTIFICATE_PASSWORD:?}" "${APPLE_SIGNING_IDENTITY:?}"
certificate_path="$RUNNER_TEMP/certificate.p12"
keychain_path="$RUNNER_TEMP/app-signing.keychain-db"
printf '%s' "$APPLE_CERTIFICATE" | base64 --decode -o "$certificate_path"
security create-keychain -p '' "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p '' "$keychain_path"
security import "$certificate_path" -P "$APPLE_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security -t cert -f pkcs12 -k "$keychain_path"
security set-key-partition-list -S apple-tool:,apple: -s -k '' "$keychain_path"
# Include the existing runner keychains so system roots remain discoverable.
security list-keychains -d user -s "$keychain_path" "$HOME/Library/Keychains/login.keychain-db"
security find-certificate -c 'Developer ID Application:' -p "$keychain_path" > "$RUNNER_TEMP/signing-certificate.pem"
bash scripts/verify-apple-certificate.sh "$RUNNER_TEMP/signing-certificate.pem" "$APPLE_SIGNING_IDENTITY"
security find-identity -v -p codesigning "$keychain_path" | grep -F "$APPLE_SIGNING_IDENTITY"
