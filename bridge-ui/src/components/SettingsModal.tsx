import { useEffect } from "react"
import { formatUnits } from "../lib/amount"
import type { RuntimeConfig } from "../lib/config"

export const SettingsModal = ({ config, showDetails, onToggleDetails, onClose }: {
  config: RuntimeConfig
  showDetails: boolean
  onToggleDetails: () => void
  onClose: () => void
}) => {
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
          <div className="setting-group"><span className="setting-label">Ethereum settlement</span><strong className="read-only-setting">Sepolia · chain {config.expectedEthereumChainId}</strong></div>
          <div className="setting-group"><span className="setting-label">Zeko signing domain</span><strong className="read-only-setting">Auro · {config.minaSigningNetworkId}</strong><p className="setting-help">Auro currently assigns the Mina testnet salt to this custom Zeko endpoint.</p></div>
          <div className="setting-group"><span className="setting-label">Experimental cap</span><strong className="read-only-setting warning">{formatUnits(BigInt(config.maxDepositWei), 18, 9)} ETH · no cancellation/refund</strong><p className="setting-help">No cancellation or refund path is available in this PoC.</p></div>
          <div className="toggle-row"><div><span className="setting-label">Route details</span><p className="setting-help">Expose the SP1 settlement route on the bridge form.</p></div><button type="button" className={`switch${showDetails ? " active" : ""}`} onClick={onToggleDetails} aria-pressed={showDetails} aria-label="Show route details"></button></div>
        </div>
      </div>
    </div>
  )
}
