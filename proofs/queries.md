# GraphQL queries — real on-chain test data

These queries fetch the data used in:
- `real_l2_inner_actions_match_onchain_state` (withdraw + bridge tests)
- `real_l1_outer_witness_matches_onchain_state` (bridge test)

---

## L2 inner actions (Zeko L2 node)

**Endpoint:** `https://testnet.zeko.io/graphql`  
**Contract:** `B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1` (L2 bridge)

```graphql
query {
  actions(
    input: {
      address: "B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1"
    }
  ) {
    blockInfo {
      stateHash
      height
    }
    actionState {
      actionStateOne
      actionStateTwo
      actionStateThree
      actionStateFour
      actionStateFive
    }
    actionData {
      accountUpdateId
      data
    }
  }
}
```

Returns a list of action groups. Each group has `actionState.actionStateOne` = state after applying these actions, and `actionData[].data` = array of field strings per action.

**Format of each action (L2 inner / withdrawal):** 3 fields
```
["0", "<aux>", "<children_digest>"]
```
- field[0] = `"0"` — discriminant (inner action)
- field[1] = aux = `Poseidon("Withdrawal_params - qFB3jXP*)", [0, amount, recipient_x])`
- field[2] = children_digest (hash of zkapp call forest; often constant per bridge config)

**State transitions used in tests** (before → fields → after):

| # | before | fields[1] (aux) | fields[2] (children_digest) | after |
|---|--------|-----------------|----------------------------|-------|
| 0 | 5338488511538591704321908497453393465896611676572626889890352515639793324972 | 13445954892259151401062147356414539397053929755454089729686468374072224770524 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 20564005778679112305921383783621393576220961645269793062533625001478041817089 |
| 1 | 20564005778679112305921383783621393576220961645269793062533625001478041817089 | 3418969254967426460902743142395488746910205347512382940433097464676038721351 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 14088641427554771616107512497342397932082101784403114407990069911207727165132 |
| 2 | 14088641427554771616107512497342397932082101784403114407990069911207727165132 | 3418969254967426460902743142395488746910205347512382940433097464676038721351 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 5592644305669396735852728084598993836947101033485055082318992298663200236730 |
| 3 | 5592644305669396735852728084598993836947101033485055082318992298663200236730 | 7290175672191916634614598157462226143709763480793909565940809202163511105802 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 7230675077846107971049681873539601135350652909070232374148538403307839283596 |
| 4 | 7230675077846107971049681873539601135350652909070232374148538403307839283596 | 23481682909396816666298220553789953254792289472463233634030406696841084292644 | 7293853241236284976483542027714912722616630571844677510574672951635140291085 | 23345261943210583986479677938738582339161417082508992471536919886924203109093 |
| 5 | 23345261943210583986479677938738582339161417082508992471536919886924203109093 | 19783371664972363249023705802644483010603479698004347610850670392839625052708 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 18067506367558727641677130278527360334316654990876259625674197924704612602695 |
| 6 | 18067506367558727641677130278527360334316654990876259625674197924704612602695 | 19783371664972363249023705802644483010603479698004347610850670392839625052708 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 2746959157610027380951551944033406547038529271116301057152331276522725315733 |
| 7 | 2746959157610027380951551944033406547038529271116301057152331276522725315733 | 27834258681202107734246517626480949164201501965735911700310484065477580173610 | 14544341622324407306183827793073118566432371121764582930297443254361206133838 | 11066481997049907237147074214507440714257448164444404179272910777489391657254 |

---

## L1 outer witness actions (Zeko actions indexer)

**Endpoint:** `https://testnet.api.actions.zeko.io/graphql`  
**Contract:** `B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2` (L1 bridge on Mina testnet)

```graphql
query {
  outerActions(address: "B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2") {
    stateBefore
    stateAfter
    action {
      ... on Witness {
        fields
        txHash
      }
      ... on Commit {
        fields
        txHash
      }
    }
  }
}
```

Returns outer (L1) actions with union type `Witness | Commit`. Witness = deposit from L1, Commit = finalization.

**Format of each Witness action (L1 outer / deposit):** 5 fields
```
["1", "<aux>", "<children_digest>", "<slot_range_lower>", "<slot_range_upper>"]
```
- field[0] = `"1"` — discriminant (outer witness)
- field[1] = aux = `Poseidon("Deposit_params - qFB3jXP*)", [0, holderL1_field, amount, recipient_x, recipient_isOdd, timeout])`
- field[2] = children_digest
- field[3] = slot_range_lower
- field[4] = slot_range_upper

**Real txn used in test:**
- txHash: `5JuHqXG3FuF9EDwQ9BwAYXaAJVDexLsbnuBX6UGVfpsFq24dkkrC`
- fields: `["1", "28349612...", "13465454...", "0", "4294967295"]`

---

## Notes

- `children_digest` is the hash of the zkapp call forest (fee payment + token transfers). It is **not** always `Field(0)` — it's a constant per standard bridge transaction type.
- The common value seen across most L2 withdrawals: `14544341622324407306183827793073118566432371121764582930297443254361206133838`
- State accumulation formula: `merkle_actions_add(prev_state, action_list_add_fields(empty, action_fields))`
  - `empty = empty_hash_with_prefix("MinaZkappActionsEmpty")`
  - `action_list_add_fields(list, fields) = hash("MinaZkappSeqEvents**", [list, hash("MinaZkappEvent******", fields)])`
  - `merkle_actions_add(state, list) = hash("MinaZkappSeqEvents**", [state, list])`
