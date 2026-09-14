import { act, renderHook, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import { NATIVE_ASSET, type BridgeAsset } from "./assets"
import { useAssetBalance } from "./useAssetBalance"

const token: BridgeAsset = {
  kind: "erc20", id: "0x0000000000000000000000000000000000000001",
  token: "0x0000000000000000000000000000000000000001",
  assetId: `0x${"11".repeat(32)}`, tokenIdL2: `0x${"22".repeat(32)}`,
  name: "USD Coin", symbol: "USDC", ethereumDecimals: 6, zekoDecimals: 6, inventoryCap: 100n
}
const deferred = () => {
  let resolve!: (value: string) => void
  let reject!: (error: Error) => void
  const promise = new Promise<string>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

describe("asset balance ownership", () => {
  it("ignores a native response after switching to an authenticated token", async () => {
    const native = deferred()
    const load = vi.fn().mockReturnValueOnce(native.promise).mockResolvedValue("25")
    const { result, rerender } = renderHook(
      ({ asset }: { asset: BridgeAsset }) => useAssetBalance("account", asset, load),
      { initialProps: { asset: NATIVE_ASSET as BridgeAsset } }
    )
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1))
    rerender({ asset: token })
    await waitFor(() => expect(result.current).toBe("25"))
    await act(async () => native.resolve("1000"))
    expect(result.current).toBe("25")
  })

  it("ignores failed requests for the previous account and clears disconnected balances", async () => {
    const old = deferred()
    const load = vi.fn().mockReturnValueOnce(old.promise).mockResolvedValue("7")
    const { result, rerender } = renderHook(
      ({ account }: { account: string | undefined }) => useAssetBalance(account, token, load),
      { initialProps: { account: "first" as string | undefined } }
    )
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1))
    rerender({ account: "second" })
    await waitFor(() => expect(result.current).toBe("7"))
    await act(async () => old.reject(new Error("old request failed")))
    expect(result.current).toBe("7")
    rerender({ account: undefined })
    expect(result.current).toBeUndefined()
  })

  it("invalidates balances when authentication or loader context changes", async () => {
    const old = deferred()
    const firstLoad = vi.fn().mockReturnValue(old.promise)
    const newLoad = vi.fn().mockResolvedValue("8")
    const { result, rerender } = renderHook(
      ({ asset, load }: { asset: BridgeAsset | undefined; load: typeof firstLoad }) =>
        useAssetBalance("account", asset, load),
      { initialProps: { asset: token as BridgeAsset | undefined, load: firstLoad } }
    )
    await waitFor(() => expect(firstLoad).toHaveBeenCalledTimes(1))
    rerender({ asset: undefined, load: firstLoad })
    await act(async () => old.resolve("999"))
    expect(result.current).toBeUndefined()
    rerender({ asset: { ...token }, load: newLoad })
    await waitFor(() => expect(result.current).toBe("8"))
  })
})
