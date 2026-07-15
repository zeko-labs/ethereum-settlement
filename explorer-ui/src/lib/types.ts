export interface Page<T> {
  items: T[];
  nextCursor: string | null;
}

export interface Summary {
  schemaVersion: 1;
  asOf: string;
  sources: { archive: boolean; gateway: boolean; ethereum: boolean };
  l2: null | {
    blockHeight: string | null;
    transactionCount: string;
    accountCount: string;
  };
  settlement: { latestSequence: string | null };
  bridge: {
    depositCount: string;
    withdrawalCount: string;
    depositedAmount: string;
  };
}

export interface BlockTransaction {
  hash: string;
  kind: string | null;
  status: string | null;
}

export interface BlockRecord {
  height: string;
  stateHash: string;
  parentHash: string;
  timestamp: string;
  chainStatus: string;
  creator: string;
  transactionCount: string;
  transaction: BlockTransaction | null;
  blockWinner?: string | null;
  ledgerHash?: string | null;
  globalSlot?: string | null;
}

export interface AccountUpdate {
  index: string;
  publicKey: string;
  tokenId: string;
  balanceChange: string;
  incrementNonce: boolean;
  callDepth: string;
  authorizationKind: string;
  useFullCommitment: boolean;
  mayUseToken: string;
}

export interface TransactionRecord {
  hash: string;
  kind: string;
  status: string;
  failureReason: string | null;
  blockHeight: string;
  stateHash: string;
  timestamp: string;
  feePayer: string;
  source: string | null;
  receiver: string | null;
  amount: string | null;
  fee: string;
  nonce: string;
  memo: string;
  accountUpdateCount: string;
  accountUpdates?: AccountUpdate[];
}

export interface SettlementRecord {
  id: string;
  source: string;
  status: string;
  createdAt: string;
  batchSequence: string | null;
  settlementCommandDigest: string | null;
  ethereumTransactionHash: string | null;
  ledgerHash: string | null;
  outerActionState: string | null;
  outerActionStateLength: string | null;
  innerActionState: string | null;
  innerActionStateLength: string | null;
  slotLower: string | null;
  slotUpper: string | null;
  innerActionRoot: string | null;
  innerActionStartIndex: string | null;
  innerActionCount: string | null;
  claimableSlot: string | null;
  confirmations: string | null;
  ethereumGasUsed: string | null;
  cycleCount: string | null;
}

export interface DepositRecord {
  nonce: string;
  token: string;
  sender: string;
  zekoRecipient: string;
  ethereumAmount: string;
  zekoAmount: string;
  timeout: string;
  ethereumTransactionHash: string;
  ethereumBlockNumber: string;
  ethereumFinalized: boolean;
  bridgeJobId: string | null;
  bridgeJobStatus: string | null;
  outerActionSequence: string | null;
  outerActionStateAfter: string | null;
  synchronizedSettlementSequence: string | null;
  status: string;
  nextAction: string | null;
  accuracyNote: string | null;
}

export interface WithdrawalRecord {
  settlementSequence: string;
  offset: number;
  globalActionIndex: string;
  recipient: string;
  amount: string;
  actionFieldsHash: string;
  siblings: string[];
  innerActionRoot: string;
  commitSlotUpper: number;
  claimableSlot: string;
  currentVirtualSlot: string;
  recipientCursor: string;
  status: string;
  nextAction: string;
  claimEthereumTransactionHash?: string;
  claimEthereumBlockNumber?: string;
  claimEthereumAmount?: string;
}

export interface AccountRecord {
  publicKey: string;
  tokenId: string;
  balance: string;
  nonce: string;
  delegate: string | null;
  lastUpdatedBlock: string;
  lastUpdatedStateHash: string;
  transactions: TransactionRecord[];
}

export interface SearchResponse {
  query: string;
  groups: {
    blocks: Array<{ height: string; stateHash: string }>;
    transactions: Array<{ hash: string; kind: string }>;
    accounts: Array<{ publicKey: string }>;
    settlements: Array<{ sequence: string; ethereumTransactionHash: string }>;
    deposits: Array<{
      nonce: string;
      ethereumTransactionHash: string;
      sender: string;
    }>;
    withdrawals: Array<{
      settlementSequence: string;
      offset: number;
      recipient: string;
    }>;
  };
}
