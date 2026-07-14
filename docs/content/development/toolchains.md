# Toolchains

Development spans the Rust/SP1 settlement repository and the companion OCaml
Zeko repository. Build them with their own pinned environments.

## Checkouts and submodules

Initialize both repositories recursively before building:

```sh
git -C ~/ethereum-settlement submodule update --init --recursive
git -C ~/zeko submodule update --init --recursive
git -C ~/zeko config --local submodule.recurse true
```

Missing Mina/proof-system submodules often look like unrelated Dune, Kimchi, or
OpenZeppelin failures.

## OCaml Zeko

The Zeko repository inherits Mina's large C/OCaml dependency graph. Use Nix;
manual opam/system dependency installation is fragile.

Required host support:

- Nix with flakes enabled
- Git submodules
- enough disk for the Nix store and Dune build tree

Open the development shell with submodules included:

```sh
cd ~/zeko
nix develop "git+file://$PWD?submodules=1" --accept-flake-config
```

Inside the shell, build the relevant paths:

```sh
dune build src/app/zeko/sequencer/cli.exe
dune build src/app/zeko/tests/ethereum_bridge_vectors.exe
```

The fixture-export scripts invoke the same Nix flake directly, so they do not
depend on a globally configured opam switch.

## Rust and SP1

This repository pins Rust 1.92 in `rust-toolchain` and SP1 6.1.0 in the guest
dependencies. Install:

- rustup with the pinned toolchain
- Succinct toolchain through `sp1up --version v6.1.0`
- `cargo-prove`
- `protoc`
- Go, used by SP1's gnark-related build components
- Docker for reproducible guest builds and local PostgreSQL

Verify the tools:

```sh
rustc --version
cargo +succinct --version
cargo prove --version
protoc --version
go version
```

Build all guests reproducibly without proving:

```sh
cargo prove build --docker --tag v6.1.0 --locked \
  --rustflags=-C,passes=lower-atomic \
  -p settlement-program -p bridge-program -p withdraw-program
```

The lower-atomic pass and the workspace's zkVM atomic shims are required by the
single-threaded RISC-V guest dependency graph.

## Solidity

Use a current Foundry toolchain and initialize the OpenZeppelin submodule:

```sh
foundryup
cd contracts
forge --version
forge build --sizes
```

Very old Foundry versions cannot parse the current submodule configuration.

## Documentation

The docs site requires Node 20 or newer and pnpm 10.28.1:

```sh
cd docs
corepack enable
pnpm install --frozen-lockfile
pnpm dev
```

## Resource safety

SP1 execution and SP1 proving are different workloads. The low-memory direct
executor can verify a real Pickles commit locally but still takes tens of
minutes of CPU time. Local proof generation is substantially heavier.

- Do not run `--prove`, Groth16/PLONK generation, or network proving as part of
  a normal build/test command.
- Use the gateway approval flow for paid network proofs.
- Use a remote Linux builder with at least 64 GB RAM for real local proving;
  128 GB or more is preferable.
- Monitor CPU, memory, and disk during long guest builds/executions.
- Remove generated Cargo/ELF build artifacts when space is exhausted, never
  fixtures, source, or vendored dependencies.
