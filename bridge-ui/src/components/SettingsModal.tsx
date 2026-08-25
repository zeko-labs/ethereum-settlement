import { useEffect } from "react"
import { ethereumNetworkName, type RuntimeConfig } from "../lib/config"
import type { MinaWalletKind } from "../lib/storage"

export const SettingsModal = ({ config, minaWallet, minaWalletBusy, showDetails, onMinaWalletChange, onToggleDetails, onClose }: {
  config: RuntimeConfig
  minaWallet: MinaWalletKind
  minaWalletBusy: boolean
  showDetails: boolean
  onMinaWalletChange: (wallet: MinaWalletKind) => void
  onToggleDetails: () => void
  onClose: () => void
}) => {
  const ethereum = ethereumNetworkName(config.expectedEthereumChainId)
  useEffect(() => {
    const close = (event: KeyboardEvent) => event.key === "Escape" && onClose()
    window.addEventListener("keydown", close)
    return () => window.removeEventListener("keydown", close)
  }, [onClose])
  return (
    <div className="settings-overlay" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <div className="modal-header"><h2 id="settings-title">Bridge settings</h2><button type="button" className="close-button" onClick={onClose} aria-label="Close settings">×</button></div>
        <div className="modal-body">
          <div className="setting-group"><span className="setting-label">Ethereum settlement</span><strong className="read-only-setting">{ethereum} · chain {config.expectedEthereumChainId}</strong></div>
          <fieldset className="setting-group wallet-setting">
            <legend className="setting-label">Mina wallet</legend>
            <div className="wallet-options" role="radiogroup" aria-label="Mina wallet provider">
              <button type="button" role="radio" disabled={minaWalletBusy} aria-checked={minaWallet === "metamask-snap"} className={`wallet-option${minaWallet === "metamask-snap" ? " selected" : ""}`} onClick={() => onMinaWalletChange("metamask-snap")}><strong>MetaMask Snap</strong><span>Use the Zeko Mina account managed inside MetaMask.</span></button>
              <button type="button" role="radio" disabled={minaWalletBusy} aria-checked={minaWallet === "auro"} className={`wallet-option${minaWallet === "auro" ? " selected" : ""}`} onClick={() => onMinaWalletChange("auro")}><strong>Auro Wallet</strong><span>Use the account and network selected in Auro.</span></button>
            </div>
            <p className="setting-help">Changing wallet disconnects the current Mina account. Connect the selected wallet again before signing.</p>
          </fieldset>
          <div className="setting-group"><span className="setting-label">Zeko signing domain</span><strong className="read-only-setting">Mina wallet · {config.minaSigningNetworkId}</strong><p className="setting-help">Auro and the MetaMask Snap currently assign the Mina testnet salt to this Zeko endpoint.</p></div>
          <div className="setting-group"><span className="setting-label">Deposit policy</span><strong className="read-only-setting warning">No cancellation/refund</strong><p className="setting-help">Deposits are not capped by the browser. Contract and protocol validation still apply.</p></div>
          <div className="toggle-row"><div><span className="setting-label">Route details</span><p className="setting-help">Expose the SP1 settlement route on the bridge form.</p></div><button type="button" className={`switch${showDetails ? " active" : ""}`} onClick={onToggleDetails} aria-pressed={showDetails} aria-label="Show route details"></button></div>
        </div>
      </div>
    </div>
  )
}
