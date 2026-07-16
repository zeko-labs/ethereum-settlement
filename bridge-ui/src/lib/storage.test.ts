import { describe, expect, it } from "vitest"
import {
  operationStorageKey,
  readOperations,
  rememberAuroConnection,
  upsertOperation,
  wasAuroConnected
} from "./storage"

describe("operation persistence", () => {
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

  it("remembers only whether Auro was previously authorized", () => {
    const storage = new Map<string, string>()
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => void storage.set(key, value),
      removeItem: (key: string) => void storage.delete(key)
    } as Storage
    expect(wasAuroConnected(adapter)).toBe(false)
    rememberAuroConnection(true, adapter)
    expect(wasAuroConnected(adapter)).toBe(true)
    expect([...storage.values()]).toEqual(["true"])
    rememberAuroConnection(false, adapter)
    expect(wasAuroConnected(adapter)).toBe(false)
  })
})
