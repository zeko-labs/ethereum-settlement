# Gateway reliability tests

Run the gateway unit tests without the SP1 SDK, native Pickles verifier, guest
build scripts, CUDA, gnark, or a proving service:

```bash
DATABASE_URL=postgres://postgres@127.0.0.1:55439/zeko_reliability_test \
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test -p zeko-proof-api \
  --no-default-features --features fake-prover-tests --bin zeko-proof-api
```

Use the isolated `zeko-reliability-postgres` hostname instead of
`127.0.0.1:55439` when running inside the persistent `zeko-dev` container.
SQLx tests create disposable databases; never point this command at a live
gateway database. Reuse the `zeko-dev:rust` tmux shell or run
`nix develop /root/nix-profiles/zeko-rust` inside that container.

This compiles the real HTTP handlers, database coordination, receipt decoders,
Ethereum RPC/signing code, and job state machine. Only `prover.rs` is replaced
with `fake_prover.rs`. Every fake prover operation fails unless the calling task
has an explicit fixture installed. Fixtures decode their supplied receipt using
the production decoder, match the proof kind and input, and provide deterministic
proof bytes and request IDs. The worker still performs its normal validation.

The suite exercises completed-proof lifetime and restart submission, a pinned
snapshot invalidated between getters, and signed successor recovery after a
local confirmation-mode reorg. These regressions use fake JSON-RPC responses
with the real worker, receipt reconciler, and PostgreSQL state transitions.

Fixtures are task scoped; concurrent tests cannot share an implicit mock. A
spawned task must install its own scope. For example, a worker test can use:

```rust,ignore
let fixture = prover::testing::Fixture::new(
    ProofKind::Settlement,
    input.clone(),
    public_values.encode(),
    program_vkey,
)?;
fixture.run(process_job(&state, job)).await?;
assert_eq!(fixture.counts().request_proof, 1);
fixture.fail_next(prover::testing::Operation::WaitProof, "temporary transport failure");
```

The fixture ignores the `proof.context` field when matching inputs because the
worker hydrates it after claiming a job. Production receipt validation remains
responsible for checking the hydrated Ethereum domain and checkpoint.

Database regressions marked ignored need an isolated PostgreSQL database. Each
coordination test creates and drops its own schema and uses separate pooled
connections to exercise concurrent admission. Run them with:

```bash
OUTER_WRITER_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55439/zeko_reliability_test \
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test -p zeko-proof-api \
  --no-default-features --features fake-prover-tests --bin zeko-proof-api \
  outer_writer::tests -- --ignored
```

The feature cannot build a server: `main.rs` has a compile error whenever
`fake-prover-tests` is enabled outside a unit-test build. Verify that guard with
the following command, which must fail with the message that the feature cannot
build a gateway server:

```bash
CARGO_BUILD_JOBS=1 cargo check -p zeko-proof-api --no-default-features \
  --features fake-prover-tests --bin zeko-proof-api
```

The default `real-prover` feature retains the existing production backend. The
two backend features are mutually exclusive. Do not run tests with default
features on the shared 16 GB development machine: those dependencies invoke
the real SP1 and native verifier build machinery.
