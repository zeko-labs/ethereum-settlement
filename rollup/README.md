# Zeko Rollup

This directory contains helper scripts for the rollup stack defined in
`../compose.rollup.yaml`.

## Requirements

- Docker
- At least 4 CPU cores and 24 GB RAM, as recommended by the Zeko operator docs.

## Start

From the repository root:

```sh
docker compose -f compose.rollup.yaml up -d
```

Generate the circuits/deploy config and the `sequencer`, `faucet`, and
`da-layer` keypairs:

```sh
docker compose -f compose.rollup.yaml exec init-config /scripts/generate-config-and-keys.sh
```

Fund the sequencer wallet on Mina Devnet. Print its public key with:

```sh
docker compose -f compose.rollup.yaml run --rm --no-deps init-config cat /data/keys/sequencer-pk
```

After funding, deploy the rollup contracts/config:

```sh
docker compose -f compose.rollup.yaml exec init-deploy /scripts/deploy.sh
```

When deployment succeeds, the sequencer GraphQL endpoint is available at:

```text
http://localhost:1923/graphql
```

## Optional configuration

If ports or network settings need to change:

```sh
cp .env.rollup.example .env.rollup
docker compose --env-file .env.rollup -f compose.rollup.yaml up -d
```

## Logs and shutdown

```sh
docker compose -f compose.rollup.yaml logs -f
docker compose -f compose.rollup.yaml down
```

Remove persistent rollup data:

```sh
docker compose -f compose.rollup.yaml down -v
```
