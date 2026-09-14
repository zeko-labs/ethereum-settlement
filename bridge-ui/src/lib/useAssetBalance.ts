import { useEffect, useState } from "react"
import type { BridgeAsset } from "./assets"

type Loader = (account: string, asset: BridgeAsset) => Promise<string>
type Balance = { account: string; asset: BridgeAsset; load: Loader; value: string }

export const useAssetBalance = (
  account: string | undefined,
  asset: BridgeAsset | undefined,
  load: Loader
): string | undefined => {
  const [balance, setBalance] = useState<Balance>()
  useEffect(() => {
    let active = true
    if (account && asset) {
      const update = (value: string) => {
        if (active) setBalance({ account, asset, load, value })
      }
      void Promise.resolve().then(() => load(account, asset)).then(update, () => update("0"))
    }
    return () => { active = false }
  }, [account, asset, load])
  return balance?.account === account && balance?.asset === asset && balance?.load === load
    ? balance.value
    : undefined
}
