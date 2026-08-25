import { useEffect } from "react"
import type { MinaWalletKind } from "../lib/storage"
import { shortAddress } from "../lib/wallets"

const snapName = (import.meta.env.VITE_MINA_SNAP_ID as string | undefined)?.startsWith("local:")
  ? "MetaMask Flask"
  : "MetaMask Snap"

const walletName = (wallet: MinaWalletKind): string =>
  wallet === "auro" ? "Auro Wallet" : snapName

export const MinaWalletModal = ({ account, minaWallet, busy, error, onConnect, onDisconnect, onClose }: {
  account?: string
  minaWallet: MinaWalletKind
  busy: boolean
  error: string
  onConnect: (wallet: MinaWalletKind) => void
  onDisconnect: () => void
  onClose: () => void
}) => {
  const title = account ? "Mina wallet" : "Connect Mina wallet"

  useEffect(() => {
    const close = (event: KeyboardEvent) => event.key === "Escape" && !busy && onClose()
    window.addEventListener("keydown", close)
    return () => window.removeEventListener("keydown", close)
  }, [busy, onClose])

  return (
    <div className="settings-overlay" onMouseDown={(event) => event.target === event.currentTarget && !busy && onClose()}>
      <div className="settings-modal mina-wallet-modal" role="dialog" aria-modal="true" aria-labelledby="mina-wallet-title">
        <div className="modal-header">
          <h2 id="mina-wallet-title">{title}</h2>
          <button type="button" className="close-button" disabled={busy} onClick={onClose} aria-label="Close Mina wallet">×</button>
        </div>
        <div className="modal-body">
          {account ? (
            <>
              <div className="connected-wallet-summary">
                <span className="setting-label">Connected with {walletName(minaWallet)}</span>
                <strong title={account}>{shortAddress(account, 12, 10)}</strong>
              </div>
              <button type="button" className="disconnect-button" disabled={busy} onClick={onDisconnect}>
                {busy ? "Disconnecting…" : "Disconnect Mina wallet"}
              </button>
              <p className="setting-help">
                {minaWallet === "metamask-snap"
                  ? `This also revokes this site's permission to use the Mina account in ${snapName}.`
                  : "Auro does not expose a dapp permission-revocation method, so this disconnects it from the bridge UI."}
              </p>
            </>
          ) : (
            <>
              <p className="wallet-choice-copy">Choose the Mina wallet you want to use with Zeko.</p>
              <div className="wallet-options wallet-connect-options">
                <button type="button" className="wallet-option" disabled={busy} onClick={() => onConnect("metamask-snap")}>
                  <strong>{snapName}</strong>
                  <span>{snapName === "MetaMask Flask" ? "Install and connect the locally served Mina Snap." : "Install or connect the published Mina Snap in MetaMask."}</span>
                </button>
                <button type="button" className="wallet-option" disabled={busy} onClick={() => onConnect("auro")}>
                  <strong>Auro Wallet</strong>
                  <span>Use the account and Zeko network selected in Auro.</span>
                </button>
              </div>
              {busy && <p className="setting-help">Opening the selected wallet…</p>}
              {error && <div className="notice error" role="alert"><span className="notice-mark">!</span><span>{error}</span></div>}
            </>
          )}
        </div>
      </div>
    </div>
  )
}
