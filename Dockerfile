FROM ghcr.io/succinctlabs/sp1:v6.1.0 AS guest-builder

WORKDIR /src
COPY . .
ARG SETTLEMENT_VK_JSON=fixtures/zeko-local-e2e/vk.serde.json
COPY ${SETTLEMENT_VK_JSON} /src/settlement-vk.serde.json

ENV CARGO_TARGET_DIR=/src/target/elf-compilation \
    SETTLEMENT_VK_JSON=/src/settlement-vk.serde.json \
    RUSTUP_TOOLCHAIN=succinct \
    RUSTC_BOOTSTRAP=1 \
    CFLAGS_riscv64im_succinct_zkvm_elf=-D__ILP32__ \
    RUSTFLAGS="-C passes=lower-atomic -C link-arg=--image-base=2013265920 -C panic=abort --cfg getrandom_backend=\"custom\" -C llvm-args=-misched-prera-direction=bottomup -C llvm-args=-misched-postra-direction=bottomup"
RUN cargo build --release --target riscv64im-succinct-zkvm-elf \
    --manifest-path program/settlement/Cargo.toml
RUN cargo build --release --target riscv64im-succinct-zkvm-elf \
    --manifest-path program/bridge/Cargo.toml
RUN cargo build --release --target riscv64im-succinct-zkvm-elf \
    --manifest-path program/withdraw/Cargo.toml

FROM golang:1.24-bookworm AS go-toolchain

FROM rust:bookworm AS api-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends clang cmake libprotobuf-dev pkg-config protobuf-compiler \
    && rm -rf /var/lib/apt/lists/* \
    && rustup toolchain install stable --profile minimal --component rustfmt

WORKDIR /src
COPY . .
COPY --from=guest-builder /src/target/elf-compilation /src/target/elf-compilation
COPY --from=guest-builder /src/settlement-vk.serde.json /src/settlement-vk.serde.json
COPY --from=go-toolchain /usr/local/go /usr/local/go

ENV PATH="/usr/local/go/bin:${PATH}" \
    SETTLEMENT_VK_JSON=/src/settlement-vk.serde.json \
    SP1_SKIP_PROGRAM_BUILD=true
RUN cargo build --locked --release -p zeko-proof-api

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=api-builder /src/target/release/zeko-proof-api /usr/local/bin/zeko-proof-api

EXPOSE 8080
CMD ["zeko-proof-api"]
