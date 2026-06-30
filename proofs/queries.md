# GraphQL queries — real on-chain test data

These curl commands fetch the exact data used in:
- `real_l2_inner_actions_match_onchain_state` (bridge + withdraw tests)
- `real_l1_outer_witness_matches_onchain_state` (bridge test)

---

## L2 inner actions (Zeko L2 node)

**Endpoint:** `https://testnet.zeko.io/graphql`  
**Contract:** `B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1` (L2 bridge)

`endActionState` pins the last of the 8 state transitions used in the tests.

```sh
curl -X POST https://testnet.zeko.io/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "{ actions(input: { address: \"B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1\", endActionState: \"11066481997049907237147074214507440714257448164444404179272910777489391657254\" }) { actionState { actionStateOne } actionData { data } } }"
  }'
```

Returns 8 action groups. Each group has:
- `actionState.actionStateOne` — accumulated state after this action
- `actionData[0].data` — array of 3 field strings `["0", "<aux>", "<children_digest>"]`

**Format of each L2 inner action:** 3 fields
```
["0", "<aux>", "<children_digest>"]
```
- `field[0]` = `"0"` — discriminant (inner action)
- `field[1]` = aux = `Poseidon("Withdrawal_params - qFB3jXP*)", [0, amount, recipient_x])`
- `field[2]` = children_digest (hash of zkapp call forest; constant `14544341622324407306183827793073118566432371121764582930297443254361206133838` for standard withdrawals)

---

## L1 outer witness action (Zeko actions indexer)

**Endpoint:** `https://testnet.api.actions.zeko.io/graphql`  
**Contract:** `B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2` (L1 bridge on Mina testnet)

`beforeState` + `afterState` pin the single deposit witness used in the test (block 530792, txn `5JuHqXG3FuF9EDwQ9BwAYXaAJVDexLsbnuBX6UGVfpsFq24dkkrC`).

```sh
curl -X POST https://testnet.api.actions.zeko.io/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "{ outerActions(input: { beforeState: \"14869234878481883326787311116385242007710904539061722321273218971438489367544\", afterState: \"20470932486817125004352886658008606971240848472715441072030772621176842217909\" }) { ... on Witness { beforeState afterState blockHeight fields txnHash slotRangeLower slotRangeUpper } ... on Commit { beforeState afterState blockHeight fields txnHash } } }"
  }'
```

Returns 1 result of type `Witness`:
```json
{
  "beforeState": "14869234878481883326787311116385242007710904539061722321273218971438489367544",
  "afterState":  "20470932486817125004352886658008606971240848472715441072030772621176842217909",
  "blockHeight": 530792,
  "txnHash":     "5JuHqXG3FuF9EDwQ9BwAYXaAJVDexLsbnuBX6UGVfpsFq24dkkrC",
  "slotRangeLower": "0",
  "slotRangeUpper": "4294967295",
  "fields": [
    "1",
    "28349612946901459216611267454622531123455255424206629024049044337709921708126",
    "13465454915859917615397187569973631104407941120704862333700387846543210055665",
    "0",
    "4294967295"
  ]
}
```

**Format of each L1 outer witness action:** 5 fields
```
["1", "<aux>", "<children_digest>", "<slot_range_lower>", "<slot_range_upper>"]
```
- `field[0]` = `"1"` — discriminant (outer witness)
- `field[1]` = aux = `Poseidon("Deposit_params - qFB3jXP*)", [0, holderL1_field, amount, recipient_x, recipient_isOdd, timeout])`
- `field[2]` = children_digest = `13465454915859917615397187569973631104407941120704862333700387846543210055665`
- `field[3]` = slot_range_lower
- `field[4]` = slot_range_upper

---

## State accumulation formula

Both programs use the same Poseidon accumulation:

```
empty        = empty_hash_with_prefix("MinaZkappActionsEmpty")
action_list  = hash("MinaZkappSeqEvents**", [empty, hash("MinaZkappEvent******", fields)])
state_after  = hash("MinaZkappSeqEvents**", [state_before, action_list])
```
