import { createHash } from "node:crypto"
import { readFile, realpath } from "node:fs/promises"
import { createRequire } from "node:module"
import path from "node:path"
import { fileURLToPath } from "node:url"

const require = createRequire(import.meta.url)
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const expected = new Map([
  ["zeko-labs-bridge-sdk-0.3.4.tgz", "e0aa4a416a2888c11adbc8834d42ef1fb76fecb7bf92c2e338daf56a853e27af"],
  ["zeko-labs-eth-bridge-sdk-0.1.0.tgz", "1828daa02d4277ccf34589600ce4a0ce65b86f01c451cc3fc0fd69a4a6d9fa67"],
  ["zeko-labs-graphql-0.3.4.tgz", "afa0ec54b60b80d78237af35c560448ddbb0b792947976949f68d026a6b535b1"]
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

const graphqlEntry = await realpath(require.resolve("@zeko-labs/graphql"))
if (!graphqlEntry.includes("@zeko-labs+graphql@file+vendor+zeko-labs-graphql-0.3.4.tgz")) {
  throw new Error(`graphql did not resolve from the vendored tarball: ${graphqlEntry}`)
}

const bridge = await import("@zeko-labs/bridge-sdk")
for (const name of ["createBridgeRuntime", "ethereumDepositAux", "buildEthereumAssetRegistryTree"]) {
  if (typeof bridge[name] !== "function") throw new Error(`Vendored bridge-sdk is missing ${name}`)
}

console.log("Vendored bridge SDK bundle verified")
