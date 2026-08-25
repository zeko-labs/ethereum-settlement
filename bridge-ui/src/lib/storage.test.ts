import { describe, expect, it } from "vitest"
import {
  operationStorageKey,
  readMinaWallet,
  readOperations,
  rememberAuroConnection,
  rememberMinaWallet,
  rememberMinaWalletConnection,
  upsertOperation,
  wasAuroConnected,
  wasMinaWalletConnected
} from "./storage"

describe("operation persistence", () => {
  it("persists a valid Mina wallet preference and ignores invalid state", () => {
    const storage = new Map<string, string>()
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => void storage.set(key, value)
    } as Storage
    expect(readMinaWallet("metamask-snap", adapter)).toBe("metamask-snap")
    rememberMinaWallet("auro", adapter)
    expect(readMinaWallet("metamask-snap", adapter)).toBe("auro")
    storage.set("zeko-eth-bridge:v1:mina-wallet", "unknown")
    expect(readMinaWallet("metamask-snap", adapter)).toBe("metamask-snap")
  })

  it("attributes the legacy connection flag to Auro", () => {
    const storage = new Map<string, string>([
      ["zeko-eth-bridge:v1:auro-connected", "true"]
    ])
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => void storage.set(key, value),
      removeItem: (key: string) => void storage.delete(key)
    } as Storage

    expect(readMinaWallet("metamask-snap", adapter)).toBe("auro")
  })

  it("keys history by chain, bridge, and wallet identity", () => {
    expect(operationStorageKey(11155111, "0xAbC", "B62:0xDEF")).toBe(
      "zeko-eth-bridge:v1:11155111:0xabc:b62:0xdef"
    )
  })

  it("recovers and updates operation identifiers without storing secrets", () => {
    const storage = new Map<string, string>()
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => void storage.set(key, value)
    } as Storage
    const operation = {
      id: "deposit:7",
      direction: "deposit" as const,
      amount: "0.1",
      recipient: "B62-recipient",
      transactionHash: "0xhash",
      createdAt: "2026-07-15T00:00:00.000Z",
      depositNonce: 7
    }
    upsertOperation("key", operation, adapter)
    expect(readOperations("key", adapter)).toEqual([operation])
    expect(storage.get("key")).not.toContain("privateKey")
  })

  it("remembers only whether the selected Mina wallet was previously authorized", () => {
    const storage = new Map<string, string>()
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => void storage.set(key, value),
      removeItem: (key: string) => void storage.delete(key)
    } as Storage
    expect(wasAuroConnected(adapter)).toBe(false)
    rememberAuroConnection(true, adapter)
    expect(wasAuroConnected(adapter)).toBe(true)
    expect(wasMinaWalletConnected("metamask-snap", adapter)).toBe(false)
    expect([...storage.values()]).toEqual(["auro"])
    rememberAuroConnection(false, adapter)
    expect(wasAuroConnected(adapter)).toBe(false)

    rememberMinaWalletConnection("metamask-snap", true, adapter)
    expect(wasMinaWalletConnected("metamask-snap", adapter)).toBe(true)
    expect(wasMinaWalletConnected("auro", adapter)).toBe(false)
  })
})
