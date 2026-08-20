#!/usr/bin/env bash

zeko_validate_da_bounds() {
  if [[ $# -ne 2 ]]; then
    echo "Usage: zeko_validate_da_bounds <node-count> <quorum>" >&2
    return 2
  fi

  local node_count=$1 quorum=$2
  [[ $node_count =~ ^[1-3]$ ]] || {
    echo "DA_NODE_COUNT must be between 1 and 3, got $node_count" >&2
    return 1
  }
  [[ $quorum =~ ^[1-3]$ && $quorum -le $node_count ]] || {
    echo "DA_QUORUM must be between 1 and DA_NODE_COUNT ($node_count), got $quorum" >&2
    return 1
  }
}

zeko_validate_da_topology() {
  if [[ $# -ne 3 ]]; then
    echo "Usage: zeko_validate_da_topology <node-count> <quorum> <public-keys>" >&2
    return 2
  fi

  local node_count=$1 quorum=$2 public_keys=$3
  local key seen
  local -a keys=() unique_keys=()
  zeko_validate_da_bounds "$node_count" "$quorum" || return
  IFS=',' read -r -a keys <<<"$public_keys"
  [[ ${#keys[@]} -eq $node_count ]] || {
    echo "DA_PUBLIC_KEYS must contain $node_count keys, got ${#keys[@]}" >&2
    return 1
  }
  for key in "${keys[@]}"; do
    [[ -n $key ]] || {
      echo "DA public keys must not be empty" >&2
      return 1
    }
    seen=false
    for unique_key in "${unique_keys[@]}"; do
      [[ $key != "$unique_key" ]] || seen=true
    done
    [[ $seen == false ]] || {
      echo "DA public keys must be distinct" >&2
      return 1
    }
    unique_keys+=("$key")
  done
}

zeko_validate_da_scenario() {
  if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "Usage: zeko_validate_da_scenario <scenario> [settlement-fixture]" >&2
    return 2
  fi

  local scenario=$1 fixture=${2:-}
  local node_count quorum commitment public_keys fixture_commitment
  [[ -f $scenario ]] || {
    echo "Missing DA scenario: $scenario" >&2
    return 1
  }
  IFS=$'\t' read -r node_count quorum commitment public_keys < <(
    jq -er '
      [select((.schemaVersion | type) == "number" and .schemaVersion >= 4
        and (.daNodeCount | type) == "number"
        and (.daQuorum | type) == "number")
       | .daNodeCount, .daQuorum,
       (.daCommitment | ascii_downcase),
       (.daPublicKeys | join(","))] | @tsv
    ' "$scenario"
  ) || {
    echo "DA scenario is missing topology metadata: $scenario" >&2
    return 1
  }
  zeko_validate_da_topology "$node_count" "$quorum" "$public_keys" || return
  [[ $commitment =~ ^0x[0-9a-f]{64}$ ]] || {
    echo "DA scenario commitment must be a 32-byte hex value" >&2
    return 1
  }

  if [[ -n $fixture ]]; then
    [[ -f $fixture ]] || {
      echo "Missing settlement fixture: $fixture" >&2
      return 1
    }
    fixture_commitment=$(jq -er \
      '.proof.binding.stateBefore.fields[6] | ascii_downcase' "$fixture") || {
      echo "Settlement fixture is missing outer-state field 6: $fixture" >&2
      return 1
    }
    [[ $fixture_commitment == "$commitment" ]] || {
      echo "DA scenario commitment does not match settlement outer-state field 6" >&2
      return 1
    }
  fi
}

zeko_read_settlement_da_commitment() {
  if [[ $# -ne 3 ]]; then
    echo "Usage: zeko_read_settlement_da_commitment <cast> <settlement> <rpc-url>" >&2
    return 2
  fi

  "$1" call "$2" 'outerState()(bytes32[8])' --rpc-url "$3" --json \
    | jq -er '.[0][6] | ascii_downcase'
}
