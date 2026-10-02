#!/usr/bin/env bash
# Check the actual leaf certificate, since both Apple intermediates share a name.
set -euo pipefail

certificate_path="${1:?Provide a PEM certificate}"
expected_fingerprint="${2:?Provide the signing certificate SHA1}"
openssl_bin="${OPENSSL_BIN:-/usr/bin/openssl}"

subject="$($openssl_bin x509 -in "$certificate_path" -noout -subject -nameopt RFC2253)"
issuer="$($openssl_bin x509 -in "$certificate_path" -noout -issuer -nameopt RFC2253)"
fingerprint="$($openssl_bin x509 -in "$certificate_path" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')"

if [[ "$subject" != *'CN=Developer ID Application:'* ]]; then
  echo '::error::Expected a Developer ID Application certificate'
  exit 1
fi
if ! grep -Eq '(^|,)OU=G2(,|$)' <<< "$issuer"; then
  echo '::error::Signing certificate must be issued by Developer ID G2'
  exit 1
fi
if [[ "$fingerprint" != "$expected_fingerprint" ]]; then
  echo '::error::Certificate fingerprint does not match APPLE_SIGNING_IDENTITY'
  exit 1
fi
"$openssl_bin" x509 -in "$certificate_path" -noout -checkend 0
if ! "$openssl_bin" x509 -in "$certificate_path" -noout -checkend 2592000; then
  echo '::warning::Developer ID certificate expires within 30 days; renew it in Apple Developer'
fi
"$openssl_bin" x509 -in "$certificate_path" -noout -subject -issuer -dates -fingerprint -sha1
