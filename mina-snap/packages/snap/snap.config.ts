import type { SnapConfig } from "@metamask/snaps-cli"

const config: SnapConfig = {
  input: "src/index.tsx",
  polyfills: {
    buffer: true
  }
}

export default config
