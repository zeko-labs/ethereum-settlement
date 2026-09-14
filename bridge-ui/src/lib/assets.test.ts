import { describe, expect, it, vi } from "vitest"
import { fetchAssetRegistrySnapshot, parseAssetRegistrySnapshot } from "./assets"

const word = (byte: string) => `0x${byte.repeat(64)}`

const snapshot = {
  schemaVersion: 1,
  root: word("1"),
  count: 1,
  depth: 16,
  records: [{
    path: Array.from({ length: 16 }, () => word("2")),
    record: {
      schemaVersion: 1,
      registryIndex: 0,
      assetId: word("3"),
      ethereumTokenAddress: "0x0000000000000000000000000000000000000001",
      tokenOwnerL2: word("4"),
      tokenIdL2: word("5"),
      decimals: 6,
      inventoryCap: "1000000000",
      mftStandardVkId: word("6"),
      vaultPublicKey: word("7"),
      universalBridgeVkId: word("8")
    }
  }]
}

describe("Ethereum asset registry", () => {
  it("converts the Actions GraphQL snapshot to exact SDK values", () => {
    const parsed = parseAssetRegistrySnapshot(snapshot)
    expect(parsed.records[0]?.record.inventoryCap).toBe(1_000_000_000n)
    expect(parsed.records[0]?.record.decimals).toBe(6)
  })

  it("rejects malformed commitments before SDK authentication", () => {
    expect(() => parseAssetRegistrySnapshot({ ...snapshot, root: "0x12" })).toThrow(/registry root/)
    expect(() => parseAssetRegistrySnapshot({
      ...snapshot,
      records: [{ ...snapshot.records[0], record: { ...snapshot.records[0]?.record, inventoryCap: "-1" } }]
    })).toThrow(/inventoryCap/)
  })

  it("fetches the latest registry through GraphQL without caching", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({
      data: { ethereumAssetRegistry: snapshot }
    }), { status: 200, headers: { "content-type": "application/json" } }))
    const parsed = await fetchAssetRegistrySnapshot("https://actions.example/graphql", fetcher)
    expect(parsed?.count).toBe(1)
    expect(fetcher).toHaveBeenCalledWith(
      "https://actions.example/graphql",
      expect.objectContaining({ method: "POST", cache: "no-store" })
    )
  })
})
