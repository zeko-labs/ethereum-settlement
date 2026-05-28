import {
  AccountUpdate,
  Field,
  Mina as LocalZekoChain,
  Poseidon,
  PrivateKey,
  PublicKey,
  Reducer,
  SmartContract,
  Struct,
  method,
} from 'o1js';

class Deposit extends Struct({
  holderAccountL1: Field,
  amount: Field,
  recipient: PublicKey,
  timeout: Field,
}) {}

function depositAux(deposit: Deposit): Field {
  return Poseidon.hashWithPrefix('Deposit_params - qFB3jXP*)', [
    Field(0),
    ...deposit.holderAccountL1.toFields(),
    ...deposit.amount.toFields(),
    ...deposit.recipient.toFields(),
    ...deposit.timeout.toFields(),
  ]);
}

class BridgeActions extends SmartContract {
  reducer = Reducer({ actionType: Field });

  @method
  async deposit(
    holderAccountL1: Field,
    amount: Field,
    recipient: PublicKey,
    timeout: Field
  ) {
    this.reducer.dispatch(
      depositAux(new Deposit({ holderAccountL1, amount, recipient, timeout }))
    );
  }
}

function zekoRecipientFromPackedAddress(packed: bigint): PublicKey {
  const oddMask = 1n << 255n;
  const isOdd = (packed & oddMask) !== 0n;
  const x = packed & (oddMask - 1n);

  return PublicKey.from({
    x: Field(x),
    isOdd,
  });
}

function getZekoActionState(address: PublicKey): Field[] {
  const account = LocalZekoChain.getAccount(address);
  const zekoActionState = account.zkapp?.actionState;

  if (zekoActionState === undefined) {
    throw new Error('missing zeko action state');
  }

  return zekoActionState;
}

function printZekoActionState(label: string, zekoActionState: Field[]) {
  console.log(label);
  zekoActionState.forEach((field, index) => {
    const hex = `0x${field.toBigInt().toString(16).padStart(64, '0')}`;
    console.log(`  [${index}] decimal=${field.toString()}`);
    console.log(`  [${index}] hex=${hex}`);
  });
}

type DepositFixture = {
  amount: bigint;
  recipient: bigint;
  timeout: bigint;
};

function testAccountKey(account: unknown): PrivateKey {
  const value = account as { key?: PrivateKey; privateKey?: PrivateKey };
  const key = value.key ?? value.privateKey;

  if (key === undefined) {
    throw new Error('local test account does not expose a private key');
  }

  return key;
}

async function main() {
  const localChain = await LocalZekoChain.LocalBlockchain({ proofsEnabled: false });
  LocalZekoChain.setActiveInstance(localChain);

  const deployer = testAccountKey(localChain.testAccounts[0]);
  const bridgeKey = PrivateKey.random();
  const bridgeAddress = bridgeKey.toPublicKey();
  const bridge = new BridgeActions(bridgeAddress);

  const holderAccountL1 = Field(1);
  const deposits: DepositFixture[] = [
    { amount: 1_000_000_000n, recipient: 0x01020304n, timeout: 3600n },
    { amount: 2_000_000_000n, recipient: 0x05060708n, timeout: 3600n },
    {
      amount: 3_000_000_000n,
      recipient: 0x80000000000000000000000000000000000000000000000000000000090a0b0cn,
      timeout: 3600n,
    },
  ];

  for (const [index, deposit] of deposits.entries()) {
    const amount = Field(deposit.amount);
    const recipient = zekoRecipientFromPackedAddress(deposit.recipient);
    const timeout = Field(deposit.timeout);

    console.log(
      `depositAux[${index}]:`,
      depositAux(
        new Deposit({
          holderAccountL1,
          amount,
          recipient,
          timeout,
        })
      ).toString()
    );
  }

  const deployTx = await LocalZekoChain.transaction(deployer.toPublicKey(), async () => {
    AccountUpdate.fundNewAccount(deployer.toPublicKey());
    await bridge.deploy();
  });
  await deployTx.sign([deployer, bridgeKey]).send();

  printZekoActionState('zekoActionState before:', getZekoActionState(bridgeAddress));

  for (const [index, deposit] of deposits.entries()) {
    const amount = Field(deposit.amount);
    const recipient = zekoRecipientFromPackedAddress(deposit.recipient);
    const timeout = Field(deposit.timeout);

    const depositTx = await LocalZekoChain.transaction(deployer.toPublicKey(), async () => {
      await bridge.deposit(holderAccountL1, amount, recipient, timeout);
    });
    await depositTx.prove();
    await depositTx.sign([deployer]).send();
    printZekoActionState(
      `zekoActionState after deposit ${index + 1}:`,
      getZekoActionState(bridgeAddress)
    );
  }

  printZekoActionState('zekoActionState after:', getZekoActionState(bridgeAddress));

  const actions = await bridge.reducer.fetchActions({
    fromActionState: Reducer.initialActionState,
  });
  console.log('fetched actions:', actions.map((actionList) => actionList.map((action) => action.toString())));
}

await main();
