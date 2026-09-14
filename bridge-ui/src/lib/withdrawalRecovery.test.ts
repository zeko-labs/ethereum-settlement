import { describe, expect, it } from "vitest"
import { activityAsset, parseTokenWithdrawalRequest, recoverWithdrawalOperation } from "./withdrawalRecovery"
import { NATIVE_ASSET, type TokenBridgeAsset } from "./assets"

const token = "0x00000000000000000000000000000000000000c0"
const assetId = `0x${"12".repeat(32)}` as const
const asset: TokenBridgeAsset = {
  kind: "erc20", id: token, token, assetId, tokenIdL2: assetId,
  name: "USD Coin", symbol: "USDC", ethereumDecimals: 6, zekoDecimals: 6, inventoryCap: 100000000n
}
const request = {
  token, assetId, globalActionIndex: 8, transactionHash: "5Jwithdraw", blockHeight: 10,
  timestamp: "1000", recipient: token, amount: "1234567",
  status: "pendingSettlement", nextAction: "waitForSettlement"
}

describe("pending token recovery", () => {
  it("recovers exact registered units and identity without local storage", () => {
    expect(recoverWithdrawalOperation(parseTokenWithdrawalRequest(request), [asset], "date")).toMatchObject({
      amount: "1.234567", globalActionIndex: 8, transactionHash: "5Jwithdraw",
      asset: { token: expect.any(String), assetId, decimals: 6, symbol: "USDC" }
    })
  })
  it("requires both authenticated token and asset identity", () => {
    expect(() => recoverWithdrawalOperation(parseTokenWithdrawalRequest(request), [], "date")).toThrow("not authenticated")
    expect(activityAsset([asset], token, `0x${"34".repeat(32)}`)).toBeUndefined()
    expect(activityAsset([], undefined)).toEqual(NATIVE_ASSET)
  })
  it("rejects malformed token identities and amounts", () => {
    expect(() => parseTokenWithdrawalRequest({ ...request, assetId: "0x12" })).toThrow()
    expect(() => parseTokenWithdrawalRequest({ ...request, amount: "-1" })).toThrow()
  })
})
