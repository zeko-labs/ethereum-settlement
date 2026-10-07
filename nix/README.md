# Ethereum settlement images and rollup preparation

This directory owns the source-local Nix packaging for the gateway, the two
UIs and the Ethereum-side bootstrap tooling. The flake keeps its existing
development shells and lockfile pins; its custom image outputs are
`x86_64-linux` only. Image references are `docker.io/zekolabs/zeko-gateway`,
`docker.io/zekolabs/zeko-bridge-ui`, `docker.io/zekolabs/zeko-explorer-ui` and
`docker.io/zekolabs/zeko-bootstrap`. Defining these references does not claim
they have been built or published.

Flake outputs (`packages.x86_64-linux`):

| Output | Kind | Content |
|---|---|---|
| `zeko-gateway-image` | image stream | generic `zeko-proof-api`, loads per-rollup artifacts at runtime |
| `zeko-bridge-ui-image` | image stream | bridge UI static assets + Node static server |
| `zeko-explorer-ui-image` | image stream | explorer UI static assets + Node static server |
| `zeko-bootstrap-image` | image stream | Ethereum half of bootstrapping: `predict-address`, `build-guests`, `vkey`, `deploy`, `forge`, `cast` |
| `zeko-sp1-toolchain` | package | the pinned SP1 `succinct` Rust toolchain used by `build-guests` |

The Zeko half of bootstrapping is not packaged here. Its one-shot steps
(signer TLS, circuit/deploy configuration, bootstrap L1 stand-in,
first-proof/genesis/VK export) run from the `zekolabs/zeko` image through its
role entrypoints (`zeko/nix/bootstrap-roles/README.md`); the signers, the DA
node and the prover are ordinary long-lived services of the `zekolabs/zeko`
and `zekolabs/zeko-da` images (native entrypoints, no scripts) that are
started before the export and keep running for the L2. This flake has no Zeko
flake input. `nix/bootstrap/compose.yaml` and `env.example` are an example
that puts both halves together; they feed the operator runbook (task 05) and
are not that runbook.

## Generic gateway image with runtime-loaded rollup artifacts

The gateway image is generic and distributable: nothing rollup-specific is
baked in. At startup `zeko-proof-api` loads the per-rollup public artifacts
from the directory named by `ZEKO_ARTIFACTS_DIR` (image default
`/var/lib/zeko-proof-api/artifacts`, declared as a volume):

```text
<artifacts>/settlement-program        SP1 settlement guest ELF
<artifacts>/bridge-program            SP1 bridge guest ELF
<artifacts>/settlement-vk.serde.json  exported public settlement VK
```

These three files are exactly what the bootstrap image's `build-guests` role
leaves in the artifacts directory (it also writes `SHA256SUMS` and
`provenance.json`; keeping those alongside is optional and ignored by the
gateway). The binary derives the native Pickles verifier from the mounted VK at
startup (the universal Pasta SRS is embedded in the binary from
`vendor/proof-systems/srs`; it is not rollup-specific) and uses the SHA-256 of
the VK file bytes as `vk_hash`. It then computes both SP1 program vkeys from
the mounted ELFs and refuses to start unless they equal the values stored in
the settlement and bridge contracts. Missing, empty or unparseable files abort
startup with an error naming the path; there is no fixture, embedded or
host-machine fallback and the gateway never selects defaults.

This is file loading and an on-chain vkey assertion only: a matching vkey does
**not** prove that a VK and ELF are cryptographically compatible or match the
target circuit. The release owner must establish and attest that relationship
from the actual circuit/guest-generation process. Do not use artifacts from
`machines`, a live deployment, or the example fixtures as a substitute for
matching target-rollup artifacts.

The artifact producer must deliver a settlement VK and settlement/bridge ELF
pair matched to the target rollup and the source revision being released. In
particular, Zeko's Ethereum bridge proxy address is a circuit configuration
input and must be fixed before generating matching OCaml circuit artifacts
(step 2 below). The contracts must be initialized from the resulting public
program keys/VK and fresh operator state. The gateway image does not produce
circuits, keys, genesis, proofs, contracts, or deployment configuration. The
current PoC is not a generic arbitrary-circuit gateway.

The gateway build compiles no SP1 guests and embeds no VK (`script/build.rs`
and the `sp1-build` dependency are gone); it needs no `SETTLEMENT_VK_JSON` or
`SP1_SKIP_PROGRAM_BUILD`. The SP1 6.1.0 `sp1-core-executor-runner` crate
normally has its build script compile `sp1-core-executor-runner-binary` with a
nested `cargo build` and embed it; inside the Nix sandbox that nested cargo
reads the vendor directory's own `.cargo/config.toml`, whose relative
`directory = "cargo-vendor-dir"` resolves to a non-existent
`cargo-vendor-dir/cargo-vendor-dir`, so `nix/docker.nix` instead builds the
real `sp1-core-executor-runner-binary` 6.1.0 crate as its own package
(`fetchCrate` + `cargoHash`, same toolchain, one job) and passes its path to
the gateway build through the runner's documented
`SP1_CORE_RUNNER_OVERRIDE_BINARY` override. The runner then compiles that
store path in instead of embedding bytes. Skipping the nested cargo is the
actual fix. The runner only spawns the helper under its native-executor
configuration; `zkapp-script` enables the sp1-sdk `profiling` feature, which
selects the portable executor and compiles the override path literal out, so
the helper is a build input and is not necessarily part of the image closure.
Nothing is replaced with a no-op. The gateway build uses its locked Cargo
vendor tree and is bounded to one Cargo/Rayon job. Loading guests at runtime
does not disable SP1 or change the application's selected proof mode. The
resulting container runs the ordinary `zeko-proof-api` executable; required
RPC, database, proof API, signer, and contract configuration/secrets are
supplied at runtime according to application source. Do not build a
fake-prover-test server image or enable unsafe Sepolia mock submission as an
image default.

The bridge and explorer images contain static application assets and a Node.js
static server. They do not bake runtime endpoint configuration. Mount a
release-specific public JSON file at `/run/zeko/runtime-config.json` in each
container, or set the corresponding `BRIDGE_UI_RUNTIME_CONFIG_FILE` or
`EXPLORER_UI_RUNTIME_CONFIG_FILE` path. The server returns that file at
`/runtime-config.json` with `Cache-Control: no-store`; the built-in local runtime
JSON is removed from the image. Configure browser URLs to be reachable by the
browser, not Compose-only DNS names. The bridge server retains COOP, COEP, and
Referrer-Policy headers; the explorer server retains Referrer-Policy. Both bind
to `0.0.0.0` by default and accept `HOST` and `PORT` runtime overrides.

## The bootstrap image

`zeko-bootstrap-image` (`docker.io/zekolabs/zeko-bootstrap`) contains, over the
shared base environment: Foundry (`forge`, `cast`), the bundled `contracts/`
workspace (`src`, `script`, `lib`, `foundry.toml`, `remappings.txt`; no
`out`/`cache`/`broadcast`), the `vkey` binary of the gateway package, the
pinned SP1 `succinct` toolchain (`nix/sp1-toolchain.nix`), `cargo` from the
repository `rust-toolchain`, `clang` (C compiler of the riscv guest target),
the nixpkgs host `cc`/binutils wrapper (`stdenv.cc`, linker of the x86_64
build scripts and proc macros; referenced by store path, not in `/bin`), the
guest sources (`Cargo.toml`,
`Cargo.lock`, `program/`, `lib/`, `crates/`, `vendor/`, plus the other member
manifests cargo needs to load the workspace) under `/opt/zeko/guest-src`, and
the locked crate set (`rustPlatform.importCargoLock`, dereferenced) under
`/opt/zeko/vendor`, referenced by absolute path from
`/opt/zeko/guest-src/.cargo/config.toml`. The entrypoint is
`nix/bootstrap/zeko-bootstrap` with one role per invocation:

| Role | Does | Needs |
|---|---|---|
| `predict-address` | `forge script PredictPocDeployment` (pure CREATE2 prediction, no RPC) | `ADMIN_ADDRESS`; writable state dir; network for the first `solc` fetch |
| `build-guests` | `cargo build --release --offline --frozen --target riscv64im-succinct-zkvm-elf` for `program/settlement` and `program/bridge` with the Dockerfile guest-builder environment (`RUSTC` = succinct rustc, `RUSTC_BOOTSTRAP=1`, `CFLAGS_…=-D__ILP32__`, `CC_riscv64im_succinct_zkvm_elf=clang`, the SP1 `RUSTFLAGS`, `CARGO_BUILD_JOBS=1`) plus the host side the SP1 image gets from its distro (`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER`, `CC_x86_64_unknown_linux_gnu`, `CC` = the wrapped host `cc`, first on `PATH`), then writes `settlement-program`, `bridge-program`, `SHA256SUMS`, `provenance.json` next to the VK | `$ZEKO_ARTIFACTS_DIR/settlement-vk.serde.json` (fails closed if missing; never a fixture); writable artifacts and state dirs; no network |
| `vkey` | `vkey --program settlement\|bridge` from the mounted artifacts | artifacts dir (read-only is enough) |
| `deploy` | `forge script DeployPoc.s.sol:DeployPoc "$@"`; sends transactions only when the operator passes `--broadcast` | DeployPoc environment; deployer key via `ZEKO_BOOTSTRAP_DEPLOYER_KEY_FILE` (secret file) or `PRIVATE_KEY`; RPC |
| `forge`, `cast` | Foundry against the bundled contracts / any cast command | |
| `help` | role list | |

Directories: `ZEKO_BOOTSTRAP_STATE_DIR=/var/lib/zeko-bootstrap` (also `HOME`)
holds the forge workspace copy (with `broadcast/` records), the `solc` cache
and the cargo target/home of `build-guests`; `ZEKO_ARTIFACTS_DIR=
/var/lib/zeko-proof-api/artifacts` is the same path the gateway uses, so one
host directory serves both. The image sets **no fixed user**: the roles write
into bind mounts owned by the host user, so run every container as that user,
`docker run --user "$(id -u):$(id -g)"` or Compose `user:`. Both directories
are root-owned `0755` in the image, so an unmounted run fails closed in the
entrypoint instead of writing into the container layer. Running as root is
not intended and leaves root-owned files in your directories.

## Image layout and loading

All four images share the base environment from `nix/base-env.nix`, modelled
on the Zeko repository's `mkBaseEnv`: `bash`, CA bundle, `openssl`, timezone
data, locales, coreutils, findutils, procps, shadow, `which`, `curl`, `jq`, tar
and gzip linked into `/bin`, `/etc` and `/share`, with `TZ=UTC`, `TZDIR` and
`SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt` set in every image.

The images are `dockerTools.streamLayeredImage` outputs: `nix build` produces
an executable script, not a tarball, and running it streams the image to
stdout. Load an image by capturing the built out-path and running that script
into `docker load`. Capture the path in a separate step: if the build fails,
an inline `"$(nix build …)" | docker load` runs an empty command and feeds
nothing into `docker load` (`permission denied` followed by
`invalid archive: does not contain a manifest.json`), hiding the real error.

```sh
out=$(nix build --no-write-lock-file --no-link --print-out-paths --max-jobs 1 --cores 1 \
  path:.#packages.x86_64-linux.<image>) && "$out" | docker load
```

`docker load < result` does not apply to these outputs.

## Fresh rollup bootstrap sequence

Source-backed steps to prepare a fresh independent rollup. No CLI commands are
invented: the Zeko one-shot roles wrap `zeko-cli`, `openssl`,
`zeko-testing-ledger` and `zeko-deployment-export`, and the long-lived
`zeko-signer`, `zeko-da` and `zeko-prover` run with their native command lines
(`zeko/nix/bootstrap-roles/README.md`); the Ethereum roles wrap `forge`,
`cast`, `cargo` and `vkey`. This is builder documentation; the operator
runbook (task 05) is separate. The commands below use the example
`nix/bootstrap/compose.yaml`; plain `docker run` equivalents follow from the
role table above.

### Operator workspace: outside every source checkout

All runtime material of the bootstrap (`.env`, wallet and Zeko private keys,
signer token, the circuit/deploy configuration, which contains fresh private
keys, signer TLS material, per-role state, the export output and the public
artifacts directory) lives in one persistent operator-owned directory
**outside all source checkouts**, called the workspace below. The reason is
mechanical: the release builds use a `path:.` flake reference, and `path:`
copies the *entire* checkout, tracked or not, into the world-readable Nix
store before any derivation-level filtering runs. No automatic secret
exclusion exists and none is claimed. `nix/docker.nix` aborts evaluation of
every output if `nix/bootstrap/{.env,secrets,config,state,tls,out}` exist in
the checkout, and the Zeko role entrypoints refuse workspace paths that look
like a checkout layout; both are guards, not substitutes for keeping the
workspace external. Nothing from the workspace ever enters a checkout.

Create the workspace as the host user who will run Compose (no root, no chown):

```sh
export ZEKO_BOOTSTRAP_WORKSPACE=$HOME/zeko-rollup-bootstrap   # any path outside all checkouts
umask 077
mkdir -p "$ZEKO_BOOTSTRAP_WORKSPACE"
cd "$ZEKO_BOOTSTRAP_WORKSPACE"
mkdir -p secrets config tls/ca tls/private out/export artifacts \
  state/signer-tls state/generate-config state/sequencer-signer state/da1-signer \
  state/da1 state/l1 state/prover state/exporter state/bootstrap
chmod 0755 tls/ca artifacts   # public: the signer certificate and the gateway artifacts
cp <ethereum-settlement checkout>/nix/bootstrap/compose.yaml .
cp <ethereum-settlement checkout>/nix/bootstrap/env.example .env
```

Edit `.env` (mode 0600) and set every value: `ZEKO_BOOTSTRAP_WORKSPACE`,
`ZEKO_BOOTSTRAP_UID`/`GID` (`id -u`/`id -g`), the three image references,
`ZEKO_SOURCE_REVISION`, `MINA_SIGNING_NETWORK_ID`, fresh random `[A-Za-z0-9]`
values for the Postgres and RabbitMQ scratch credentials (broker user not
`guest`), and `ADMIN_ADDRESS` after step 1.

Ownership model: every custom container runs as `ZEKO_BOOTSTRAP_UID:GID`, the
workspace owner, so secrets stay 0600, directories 0700, bind-mounted state is
writable without chown, privileged init or Docker-in-Docker, and no image
declares a volume. Each role mounts only what it needs: signers get
`tls/private` read-only, clients get `tls/ca` only, the prover and exporter
get `config/` read-only, only the exporter mounts `out/export`, only
`build-guests` mounts `artifacts/` writable. The signer and DA containers
read their `/run/secrets` files into the environment variables the Zeko
binaries expect (`MINA_PRIVATE_KEY`, `ZEKO_SIGNER_AUTH_TOKEN`) inside a
`bash -c` entrypoint; the values never appear in `.env`, arguments or logs.
`postgres:16` and `rabbitmq:latest` keep their own internal users and tmpfs
data. Back up `secrets/`, `config/`, `out/export/` and `state/da1`,
`state/exporter` (the DA database the L2 keeps using and the exporter's
sequencer state) offline; never place them in a repository.

### Step 1: Admin wallet key

Generate a fresh Ethereum admin/deployer keypair with `cast` from the bootstrap
image. The admin address controls the PoC factory and the deployed proxies.
Write the key only into the workspace secrets directory:

```sh
cd "$ZEKO_BOOTSTRAP_WORKSPACE"
(umask 077 && docker run --rm --user "$(id -u):$(id -g)" docker.io/zekolabs/zeko-bootstrap:latest \
  cast wallet new --json > secrets/admin-wallet.json)
(umask 077 && jq -r '.[0].private_key' secrets/admin-wallet.json > secrets/deployer_key)
jq -r '.[0].address' secrets/admin-wallet.json     # ADMIN_ADDRESS for .env
```

### Step 2: Predict the CREATE2 contract addresses

`contracts/script/PocDeploymentConfig.sol` `_predict` is `pure`: the PoC
factory addresses depend only on the admin address and fixed bytecode salts,
so prediction needs no RPC and no artifacts. The bridge proxy address is the
OCaml circuit's `ethereum_holder_account_l1` input and must be known before
the circuits are compiled and the VK exported:

```sh
docker compose --profile bootstrap run --rm predict-address
```

Every `zeko-bootstrap` service in the example Compose fixes its role in
`entrypoint:` (`/bin/zeko-bootstrap predict-address`, `build-guests`, `vkey`,
`deploy`) with an empty `command:`, so `predict-address` and `build-guests`
run with no arguments and anything after the service name in
`docker compose run` (steps 5 and 6) is appended to the role rather than
replacing it. Record `BRIDGE_CONTRACT_ADDRESS` from the output. Forge fetches the pinned
`solc` into `state/bootstrap` on first use, which is why this role has egress.

### Step 3: Zeko configuration, long-lived services and export (zeko images)

Create `secrets/sequencer_key`, `secrets/da1_key` (fresh Zeko Base58 private
keys) and `secrets/signer_auth_token` (random string), each 0600. Then:

```sh
docker compose --profile bootstrap run --rm signer-tls                      # tls/ca, tls/private
docker compose --profile bootstrap run --rm generate-config 0x<bridge-proxy-address>
docker compose up -d                                                        # long-lived services
docker compose --profile bootstrap run --rm exporter                        # out/export must be empty
docker compose --profile bootstrap stop bootstrap-l1                        # export-only stand-in
```

`generate-config` writes `config/circuits-config.json` and
`config/deploy-config.json` (private; umask 077) bound to the predicted bridge
proxy address; the exporter refuses to start unless those files are named, and
the prover service names them explicitly, so Zeko's embedded test keys cannot
be used by accident. `docker compose up -d` starts the long-lived services:
Postgres, RabbitMQ, the sequencer signer, the DA signer, the DA node and the
prover. These are not bootstrap tools: they are the services the L2 runs on,
and they keep running after the export (the sequencer and the gateway join
them in the operator runbook). The DA node starts once its signer is healthy.
The prover compiles the circuits first; its healthcheck reports ready only
once it holds a broker connection, allowing 8 h plus ten 30 s retries
(`docker compose ps`, `docker compose logs prover`). The exporter waits for the
healthy prover, signer and DA node and starts the bootstrap L1 stand-in
`zeko-testing-ledger` itself, then runs the first settlement proof against
them; `out/export/` holds `deployment-manifest.json`, `genesis-ledger.json`,
`settlement.json`, `vk.serde.json`, `settlement-vk.serde.json` (same bytes),
`proof.serde.json`, `public_input_skeleton.json` and `app_statement.json`.
Keep this directory durable: contract initialization (`DeployPoc.s.sol`
initial outer state, action state, fork slot) comes from it. The genesis diff
posted during the export stays in `state/da1`, the DA database the L2 keeps
using; do not reset it. Stop the stand-in afterwards as shown; the long-lived
services stay up. To re-run, empty `out/export/` yourself; the exporter never
overwrites.

### Step 4: Build the SP1 guests (bootstrap image)

Copy the exported **public** VK into the artifacts directory and compile the
guests against it inside the container (offline; pinned `succinct`
toolchain `succinct-1.93.0-64bit`, the exact asset sp1-cli 6.1.0 installs,
fixed-output hash measured with `nix store prefetch-file`; same target and
flags as the `Dockerfile` guest-builder stage; one cargo job):

```sh
cp out/export/settlement-vk.serde.json artifacts/settlement-vk.serde.json
chmod 0644 artifacts/settlement-vk.serde.json
docker compose --profile bootstrap run --rm build-guests
(cd artifacts && sha256sum -c SHA256SUMS)
```

`artifacts/` now holds `settlement-program`, `bridge-program`,
`settlement-vk.serde.json`, `SHA256SUMS` and `provenance.json` (toolchain
versions, settlement source revision when the image was built from a clean
git reference, digests). The role refuses to run without the VK and refuses to
overwrite existing ELFs; the cargo target directory persists in
`state/bootstrap/target` so a rebuild is incremental. This is a long,
memory-heavy compile (plan for 16 GB+). The directory is public data: keep it
`0755` with `0644` files, since the gateway container reads it as a different
uid (`65532`).

### Step 5: SP1 program verification keys

```sh
SETTLEMENT_VKEY=$(docker compose --profile bootstrap run --rm vkey --program settlement)
BRIDGE_VKEY=$(docker compose --profile bootstrap run --rm vkey --program bridge)
VK_HASH=0x$(sha256sum artifacts/settlement-vk.serde.json | cut -d' ' -f1)
```

The `vkey` service's entrypoint is `/bin/zeko-bootstrap vkey`, so
`--program settlement|bridge` reaches the `vkey` binary as its arguments.

The gateway computes the same vkeys from the mounted directory at every start
and refuses to run if they differ from the contract values.

### Step 6: Deploy the contracts

`DeployPoc.s.sol` reads every parameter from the environment. Put the public
values into `config/deploy.env` (0600): `SP1_VERIFIER_ADDRESS` (official SP1
v6.1 Groth16 verifier on Sepolia), `SETTLEMENT_PROGRAM_VKEY`,
`BRIDGE_PROGRAM_VKEY`, `SETTLEMENT_VK_HASH` (step 5), `UPGRADER_ADDRESS`,
`GATEWAY_PROVER_ADDRESS`, `MINA_SIGNING_NETWORK_ID`, `INITIAL_OUTER_STATE_0…7`
and `INITIAL_OUTER_ACTION_STATE` from `out/export/settlement.json`, `FORK_SLOT`
and the other values listed in `contracts/script/DeployPoc.s.sol` and
`TESTNET_POC.md`. The deployer key is read from `secrets/deployer_key` by the
role, never from `.env` or the command line. A run without `--broadcast` is a
dry run:

```sh
docker compose --profile bootstrap run --rm deploy --rpc-url <sepolia-rpc-url>
docker compose --profile bootstrap run --rm deploy --rpc-url <sepolia-rpc-url> --broadcast
```

The `deploy` service's entrypoint is `/bin/zeko-bootstrap deploy`, so
`--rpc-url` and `--broadcast` are forwarded to `forge script`. Forge's
`broadcast/` records persist in `state/bootstrap/forge-workspace`.

### Step 7: Run the gateway with the mounted artifacts

Mount `artifacts/` read-only at `/var/lib/zeko-proof-api/artifacts` (or point
`ZEKO_ARTIFACTS_DIR` elsewhere) in every gateway container, together with the
runtime configuration and secrets described by the application source. On
start the gateway logs `loaded settlement artifacts` with the VK hash, asserts
both program vkeys against the contracts and only then serves. If the
directory is missing, the error names the directory and the three expected
files. The sequencer of the production stack points `--l1-uri` at the
gateway's `/graphql` and uses the signer, DA node and prover already running
from step 3; the remainder of the stack (sequencer and gateway services,
configuration, database initialization) belongs to the operator runbook, not
this file.

## User-owned image builds

The five settlement outputs need no rollup artifacts and can be built on
`x86_64-linux` from this checkout at any time. `path:.` copies the whole
checkout, which is why no private material may be there. The gateway and
bootstrap builds are heavy (SP1 Rust compilation; 64 GB+ recommended, at
minimum 16 GB with one job). Each image command captures the out-path first
and stops before `docker load` if the build fails:

```sh
cd <ethereum-settlement checkout>
for image in zeko-gateway-image zeko-bridge-ui-image zeko-explorer-ui-image zeko-bootstrap-image; do
  out=$(nix build --no-write-lock-file --no-link --print-out-paths --max-jobs 1 --cores 1 \
    "path:.#packages.x86_64-linux.$image") || break
  "$out" | docker load || break
done
nix build --no-write-lock-file --no-link --max-jobs 1 --cores 1 path:.#packages.x86_64-linux.zeko-sp1-toolchain
```

Single-image form:

```sh
out=$(nix build --no-write-lock-file --no-link --print-out-paths --max-jobs 1 --cores 1 \
  path:.#packages.x86_64-linux.zeko-bootstrap-image) && "$out" | docker load
```

These commands build image streams only. They do not publish images or
establish runtime, contract, circuit, or deployment correctness. Tagging
(`docker.io/zekolabs/<name>:<release-tag>`), publication and operator runtime
configuration remain release/deployment responsibilities.
