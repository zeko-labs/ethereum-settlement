const explorerTransactions = [
  { hash: "5JtY7ZkLwcDaV8Njzv9zc1H7EmL4WQwNwJ8eqZ9GzGCxM2nu82qR", block: "18,492", type: "zkApp", from: "B62qkJzX…x7Kj", status: "Applied", age: "12 sec" },
  { hash: "5JuQFcHqcb6J9GzULspsf1dXCrVh9zLRYoR4x5K3zpTJdY4cFhhj", block: "18,491", type: "Payment", from: "B62qib7T…p4Lm", status: "Applied", age: "26 sec" },
  { hash: "5Jv21Qu2vxM4LLEeQZrkfH8aF6Uj2QJwS1txu6hVjZNfYJmS9Rxn", block: "18,490", type: "zkApp", from: "B62qmVw9…8zAa", status: "Failed", age: "41 sec" },
  { hash: "5Juy4Ex1h3QmGLwSUy83AgmTz1nXrX83iRjzbTBqPSu4S7dq11Ef", block: "18,489", type: "Delegation", from: "B62qkXw2…nQ9s", status: "Applied", age: "58 sec" },
  { hash: "5Jt3u6LmE6c4kB1pSu8AbWGZo7bvYCyPbFxWb7i9tqDJkE4z2T6d", block: "18,488", type: "zkApp", from: "B62qib7T…p4Lm", status: "Applied", age: "1 min" }
];

const explorerSettlements = [
  { id: "#0284", slot: "91,884–91,948", tx: "0x25e2…b8c1", status: "Confirmed", blocks: "64", age: "2 min", bridge: "3 deposits · 1 withdrawal" },
  { id: "#0283", slot: "91,820–91,883", tx: "0x6ad1…074f", status: "Confirmed", blocks: "63", age: "8 min", bridge: "No bridge actions" },
  { id: "#0282", slot: "91,756–91,819", tx: "0xc421…e910", status: "Confirmed", blocks: "64", age: "15 min", bridge: "2 withdrawals" },
  { id: "#0285", slot: "91,949–92,012", tx: "Pending", status: "Proving", blocks: "64", age: "Now", bridge: "2 deposits" }
];

const explorerBridge = [
  { id: "Deposit #147", route: "Ethereum → Zeko", account: "0x8Fb2…19A0", amount: "0.084 ETH", status: "Ready to finalize", age: "3 min", kind: "deposit" },
  { id: "Withdrawal 282:3", route: "Zeko → Ethereum", account: "0x71aC…0b2E", amount: "0.012 ETH", status: "Waiting for delay", age: "15 min", kind: "withdrawal" },
  { id: "Deposit #146", route: "Ethereum → Zeko", account: "0x11D7…c902", amount: "0.025 ETH", status: "Synchronized", age: "18 min", kind: "deposit" },
  { id: "Withdrawal 281:0", route: "Zeko → Ethereum", account: "0x4F82…88C1", amount: "0.006 ETH", status: "Claimed", age: "23 min", kind: "withdrawal" }
];

Object.assign(window, { explorerTransactions, explorerSettlements, explorerBridge });
