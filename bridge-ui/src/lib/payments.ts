import { FungibleToken } from "mina-fungible-token"
import {
  AccountUpdate,
  fetchAccount,
  Mina,
  PublicKey,
  UInt64
} from "o1js"
import type { RuntimeConfig } from "./config"
import {
  createAuroSigner,
  isValidZekoAddress
} from "./bridge"
import {
  ensureAuroPoCNetwork,
  getAuroProvider,
  isProviderError,
  type AuroProvider
} from "./wallets"

export type NativePaymentInput = {
  recipient: string
  amountMina: string
  feeMina: string
  memo?: string
}

export type MftPaymentInput = {
  tokenOwner: string
  recipient: string
  amountBaseUnits: string
  feeNanomina: string
}

const positiveDecimal = (value: string, label: string): number => {
  if (!/^(?:0|[1-9]\d*)(?:\.\d{1,9})?$/u.test(value)) {
    throw new Error(`${label} must be a decimal with at most 9 fractional digits.`)
  }
  const parsed = Number(value)
  if (!Number.isFinite(parsed) || parsed <= 0) throw new Error(`${label} must be greater than zero.`)
  return parsed
}

const unsignedInteger = (value: string, label: string, allowZero = false): bigint => {
  if (!/^\d+$/u.test(value)) throw new Error(`${label} must use exact integer base units.`)
  const parsed = BigInt(value)
  if (allowZero ? parsed < 0n : parsed <= 0n) throw new Error(`${label} must be greater than zero.`)
  if (parsed > (1n << 64n) - 1n) throw new Error(`${label} exceeds Mina's UInt64 range.`)
  return parsed
}

export const sendNativePayment = async ({
  config,
  input,
  provider = getAuroProvider()
}: {
  config: RuntimeConfig
  input: NativePaymentInput
  provider?: AuroProvider
}): Promise<string> => {
  if (!await isValidZekoAddress(input.recipient)) throw new Error("Enter a valid B62 recipient.")
  const amount = positiveDecimal(input.amountMina, "Amount")
  const fee = positiveDecimal(input.feeMina, "Fee")
  await ensureAuroPoCNetwork(provider, config)
  if (!provider.sendPayment) throw new Error("The selected Mina wallet cannot send payments.")
  const result = await provider.sendPayment({
    to: input.recipient,
    amount,
    fee,
    ...(input.memo ? { memo: input.memo } : {})
  })
  if (result instanceof Error) throw result
  if (isProviderError(result)) throw new Error(result.message ?? `Mina wallet error ${result.code}`)
  if (!result.hash) throw new Error("The Mina wallet returned no payment hash.")
  return result.hash
}

let compileMftPromise: Promise<unknown> | undefined

const compileMft = async (): Promise<void> => {
  compileMftPromise ??= FungibleToken.compile().catch((error: unknown) => {
    compileMftPromise = undefined
    throw error
  })
  await compileMftPromise
}

export const sendMftPayment = async ({
  config,
  sender,
  input,
  provider = getAuroProvider()
}: {
  config: RuntimeConfig
  sender: string
  input: MftPaymentInput
  provider?: AuroProvider
}): Promise<string> => {
  if (!await isValidZekoAddress(input.tokenOwner)) throw new Error("Enter a valid MFT token-owner address.")
  if (!await isValidZekoAddress(input.recipient)) throw new Error("Enter a valid B62 recipient.")
  const amount = unsignedInteger(input.amountBaseUnits, "Amount")
  const fee = unsignedInteger(input.feeNanomina, "Fee")
  await ensureAuroPoCNetwork(provider, config)

  Mina.setActiveInstance(Mina.Network({
    mina: config.sequencerGraphqlUrl,
    archive: config.zekoArchiveGraphqlUrl,
    networkId: config.minaSigningNetworkId
  }))
  const senderKey = PublicKey.fromBase58(sender)
  const recipientKey = PublicKey.fromBase58(input.recipient)
  const token = new FungibleToken(PublicKey.fromBase58(input.tokenOwner))
  const tokenId = token.deriveTokenId()
  const [senderAccount, tokenOwnerAccount, recipientAccount] = await Promise.all([
    fetchAccount({ publicKey: senderKey }),
    fetchAccount({ publicKey: token.address }),
    fetchAccount({ publicKey: recipientKey, tokenId })
  ])
  if (senderAccount.error) throw new Error(`Could not load the fee payer: ${senderAccount.error.statusText}`)
  if (tokenOwnerAccount.error) throw new Error(`Could not load the MFT contract: ${tokenOwnerAccount.error.statusText}`)
  if (recipientAccount.error && recipientAccount.error.statusCode !== 404) {
    throw new Error(`Could not load the recipient token account: ${recipientAccount.error.statusText}`)
  }
  const recipientIsNew = recipientAccount.account === undefined

  await compileMft()
  const transaction = await Mina.transaction({ sender: senderKey, fee: fee.toString() }, async () => {
    if (recipientIsNew) AccountUpdate.fundNewAccount(senderKey)
    await token.transfer(senderKey, recipientKey, UInt64.from(amount))
  })
  const proved = await transaction.prove()
  const signed = await createAuroSigner(provider, config)(proved)
  const pending = await signed.send()
  if (pending.status === "rejected") {
    throw new Error(pending.errors.join("; ") || "The Mina node rejected the MFT transfer.")
  }
  return pending.hash
}
