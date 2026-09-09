#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
temporary_directory=$(mktemp -d)
trap 'rm -rf -- "$temporary_directory"' EXIT

certificate="$temporary_directory/signer.crt"
private_key="$temporary_directory/signer.key"
"$ROOT/tools/generate-local-signer-certificate.sh" \
  "$certificate" "$private_key" 'DNS:localhost,IP:127.0.0.1'

openssl x509 -checkend $((7 * 24 * 60 * 60)) -noout -in "$certificate" >/dev/null
openssl verify -CAfile "$certificate" "$certificate" >/dev/null
[[ $(stat -c '%a' "$certificate") == 600 ]]
[[ $(stat -c '%a' "$private_key") == 600 ]]

if ZEKO_SIGNER_CERT_DAYS=invalid \
  "$ROOT/tools/generate-local-signer-certificate.sh" \
    "$temporary_directory/bad.crt" "$temporary_directory/bad.key" 2>/dev/null; then
  echo "certificate generator accepted an invalid lifetime" >&2
  exit 1
fi

echo "local signer certificate checks passed"
