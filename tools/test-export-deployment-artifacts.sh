#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

cat >"$TMP_DIR/mock-nix" <<'MOCK_NIX'
#!/usr/bin/env bash
set -euo pipefail
while [[ $1 != -c ]]; do shift; done
shift
[[ $1 == src/app/zeko/sequencer/run-deployment-export.sh ]]
output_directory=$2
[[ $3 == 1 && $4 == 1 ]]
commitment=0x$(printf 'ab%.0s' {1..32})
genesis=0x$(printf 'cd%.0s' {1..32})
pause_key=0x$(printf 'ef%.0s' {1..32})
jq -n --arg commitment "$commitment" --arg genesis "$genesis" '
  {schemaVersion:1,outerPublicKey:"B62outer",outerActionState:$commitment,
   genesisLedgerField:$genesis,sequencerPublicKey:"B62sequencer",
   daPublicKeys:["B62da1"],daNodeCount:1,daQuorum:1,
   daCommitment:$commitment}' >"$output_directory/deployment-manifest.json"
if [[ ${MOCK_INVALID_MANIFEST:-false} == true ]]; then
  jq '.daQuorum = 2' "$output_directory/deployment-manifest.json" \
    >"$output_directory/deployment-manifest.tmp.json"
  mv "$output_directory/deployment-manifest.tmp.json" \
    "$output_directory/deployment-manifest.json"
fi
jq -n '[[0,{public_key:"B62inner"}]]' \
  >"$output_directory/genesis-ledger.json"
jq -n --arg commitment "$commitment" --arg genesis "$genesis" \
  --arg pauseKey "$pause_key" '
  {outerAccountPublicKey:"B62outer",proof:{vkJson:"{}",proofJson:"{}",
   publicInputSkeletonJson:"{}",appStatementJson:"[]",binding:{stateBefore:{
   fields:[$pauseKey,"0x00",$genesis,"0x00","0x00","0x00",$commitment,
   "0x00"]},accountUpdateBody:{fieldElements:
   ([range(0;36) | "0x00"] + [$commitment])}}}}' \
  >"$output_directory/settlement-0000000000-test.json"
MOCK_NIX
chmod +x "$TMP_DIR/mock-nix"

mkdir -p "$TMP_DIR/zeko/src/app/zeko"
cp /dev/null "$TMP_DIR/zeko/src/app/zeko/.keep"
git -C "$TMP_DIR/zeko" init -q
git -C "$TMP_DIR/zeko" config user.name test
git -C "$TMP_DIR/zeko" config user.email test@example.invalid
git -C "$TMP_DIR/zeko" add src/app/zeko/.keep
git -C "$TMP_DIR/zeko" commit -qm init
printf '{}\n' >"$TMP_DIR/circuits.json"
printf '{}\n' >"$TMP_DIR/deploy.json"

cat >"$TMP_DIR/fixture.env" <<ENV
ZEKO_CIRCUITS_CONFIG=$TMP_DIR/circuits.json
ZEKO_DEPLOY_CONFIG=$TMP_DIR/deploy.json
ZEKO_TEST_SEQUENCER_SIGNER_PRIVATE_KEY=sequencer-private
DA1_SIGNER_PRIVATE_KEY=da-private
DA_NODE_COUNT=1
DA_QUORUM=1
ENV

output_directory="$TMP_DIR/fixture"
ZEKO_ROOT="$TMP_DIR/zeko" POC_ENV_FILE="$TMP_DIR/fixture.env" \
NIX="$TMP_DIR/mock-nix" \
  "$ROOT/tools/export-deployment-artifacts.sh" "$output_directory" \
  >"$TMP_DIR/result.json"

for file in deployment-manifest.json genesis-ledger.json settlement.json \
  vk.serde.json proof.serde.json public_input_skeleton.json \
  app_statement.json; do
  [[ -f "$output_directory/$file" ]]
done
[[ $(jq -r '.daNodeCount' "$TMP_DIR/result.json") == 1 ]]
[[ $(jq -r '.zekoSourceRevision | length' \
  "$output_directory/deployment-manifest.json") == 40 ]]
[[ $(jq -r '.settlementVkSha256 | length' \
  "$output_directory/deployment-manifest.json") == 64 ]]
[[ $(find "$output_directory" -maxdepth 1 -name 'settlement-*' | wc -l) == 0 ]]

if ZEKO_ROOT="$TMP_DIR/zeko" POC_ENV_FILE="$TMP_DIR/fixture.env" \
  NIX="$TMP_DIR/mock-nix" MOCK_INVALID_MANIFEST=true \
  "$ROOT/tools/export-deployment-artifacts.sh" "$TMP_DIR/bad-fixture" \
  >/dev/null 2>&1; then
  echo "Exporter accepted a mismatched fixture manifest" >&2
  exit 1
fi

if ZEKO_ROOT="$TMP_DIR/zeko" POC_ENV_FILE="$TMP_DIR/fixture.env" \
  NIX="$TMP_DIR/mock-nix" \
  "$ROOT/tools/export-deployment-artifacts.sh" "$output_directory" \
  >/dev/null 2>&1; then
  echo "Exporter unexpectedly overwrote a fixture" >&2
  exit 1
fi

echo "Deployment fixture export passed."
