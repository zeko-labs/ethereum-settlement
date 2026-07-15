import { createHash } from "node:crypto"
import { readFile, realpath } from "node:fs/promises"
import { createRequire } from "node:module"
import path from "node:path"
import { fileURLToPath } from "node:url"

const require = createRequire(import.meta.url)
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const expected = new Map([
  ["zeko-labs-bridge-sdk-0.3.4.tgz", "9d1b1e6b277340d2c3624a26d2b376637ac9c1273b4e98c4df9480961d60b192"],
  ["zeko-labs-eth-bridge-sdk-0.1.0.tgz", "2e280e62a02f75b6e5a88847a0be79bdf80e3dd9abee7e4d36757f018bafc72d"]
])

for (const [name, digest] of expected) {
  const contents = await readFile(path.join(root, "vendor", name))
  const actual = createHash("sha256").update(contents).digest("hex")
  if (actual !== digest) throw new Error(`${name} digest mismatch: ${actual}`)
}

const bridgeEntry = await realpath(require.resolve("@zeko-labs/bridge-sdk"))
if (!bridgeEntry.includes("@zeko-labs+bridge-sdk@file+vendor+zeko-labs-bridge-sdk-0.3.4.tgz")) {
  throw new Error(`bridge-sdk did not resolve from the vendored tarball: ${bridgeEntry}`)
}

const ethRequire = createRequire(require.resolve("@zeko-labs/eth-bridge-sdk"))
const transitiveBridgeEntry = await realpath(ethRequire.resolve("@zeko-labs/bridge-sdk"))
if (!transitiveBridgeEntry.includes("@zeko-labs+bridge-sdk@file+vendor+zeko-labs-bridge-sdk-0.3.4.tgz")) {
  throw new Error(`eth-bridge-sdk resolved a non-vendored bridge-sdk: ${transitiveBridgeEntry}`)
}

const bridge = await import("@zeko-labs/bridge-sdk")
for (const name of ["createBridgeRuntime", "ethereumDepositAux"]) {
  if (typeof bridge[name] !== "function") throw new Error(`Vendored bridge-sdk is missing ${name}`)
}

console.log("Vendored bridge SDK pair verified")
