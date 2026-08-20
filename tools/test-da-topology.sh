#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/da-topology.sh"

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

expect_failure() {
  if "$@" 2>"$TMP_DIR/error.log"; then
    echo "Command unexpectedly succeeded: $*" >&2
    exit 1
  fi
}

zeko_validate_da_topology 1 1 key-1
zeko_validate_da_topology 3 2 key-1,key-2,key-3
expect_failure zeko_validate_da_topology 1 2 key-1
expect_failure zeko_validate_da_topology 2 1 key-1
expect_failure zeko_validate_da_topology 2 1 key-1,key-1

commitment=0x$(printf 'ab%.0s' {1..32})
jq -n --arg commitment "$commitment" \
  '{schemaVersion:4,daNodeCount:1,daQuorum:1,daCommitment:$commitment,
    daPublicKeys:["key-1"]}' >"$TMP_DIR/scenario.json"
jq -n --arg commitment "$commitment" \
  '{proof:{binding:{stateBefore:{fields:["0x00","0x00","0x00","0x00",
    "0x00","0x00",$commitment,"0x00"]}}}}' >"$TMP_DIR/settlement.json"
zeko_validate_da_scenario "$TMP_DIR/scenario.json" "$TMP_DIR/settlement.json"
jq '.schemaVersion = 3' "$TMP_DIR/scenario.json" >"$TMP_DIR/old-scenario.json"
expect_failure zeko_validate_da_scenario "$TMP_DIR/old-scenario.json"

jq '.proof.binding.stateBefore.fields[6] =
  "0xcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"' \
  "$TMP_DIR/settlement.json" >"$TMP_DIR/mismatch.json"
expect_failure zeko_validate_da_scenario \
  "$TMP_DIR/scenario.json" "$TMP_DIR/mismatch.json"

mock_cast() {
  jq -n --arg commitment "$commitment" \
    '[["0x00","0x00","0x00","0x00","0x00","0x00",$commitment,"0x00"]]'
}
[[ $(zeko_read_settlement_da_commitment mock_cast settlement rpc) == \
  "$commitment" ]]

echo "DA topology validation passed."
