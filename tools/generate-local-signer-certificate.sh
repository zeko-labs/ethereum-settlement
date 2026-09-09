#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <certificate-path> <private-key-path> [subject-alt-name] [common-name]" >&2
  exit 2
}

[[ $# -ge 2 && $# -le 4 ]] || usage
certificate_path=$1
private_key_path=$2
subject_alt_name=${3:-DNS:localhost}
common_name=${4:-localhost}
valid_days=${ZEKO_SIGNER_CERT_DAYS:-30}

[[ $valid_days =~ ^[1-9][0-9]*$ ]] || {
  echo "ZEKO_SIGNER_CERT_DAYS must be a positive integer" >&2
  exit 2
}
[[ $common_name =~ ^[A-Za-z0-9._-]+$ ]] || {
  echo "Common name contains unsupported characters" >&2
  exit 2
}
command -v openssl >/dev/null || {
  echo "Missing command: openssl" >&2
  exit 1
}
mkdir -p "$(dirname "$certificate_path")" "$(dirname "$private_key_path")"
umask 077
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$private_key_path" -out "$certificate_path" \
  -days "$valid_days" -subj "/CN=$common_name" \
  -addext "subjectAltName=$subject_alt_name" >/dev/null 2>&1
chmod 0600 "$certificate_path" "$private_key_path"
