# turk-binary docs

turk-binary watches Solana DEX pools and (later) trades price gaps between them.

| Page                                 | What it covers                                                    |
| ------------------------------------ | ----------------------------------------------------------------- |
| [architecture.md](architecture.md)   | How the crates fit together and how data flows                    |
| [configuration.md](configuration.md) | Every config key and environment variable                         |
| [dexes.md](dexes.md)                 | Supported DEXes, their names in config, and how each was verified |
| [router.md](router.md)               | The on-chain router program: instructions, wire format, errors    |

## Quick start

```sh
cp config.example.toml config.toml
cp .env.example .env      # fill in TB_RPC_URL, TB_GRPC_URL, TB_GRPC_X_TOKEN
just watch
```

`just` loads `.env` by itself, so the endpoints never need to be exported in your shell.

`watch` is read-only. It never signs or sends a transaction. `just serve` does the same and also answers `POST /quote`, `/swap-instructions` and `/swap` on `127.0.0.1:8080` and `/health`, `/ready` on `127.0.0.1:9100` (see [architecture.md](architecture.md#http-api)). It prices routes and builds unsigned transactions for the user's wallet to sign; it never signs or sends one.

Run `just probe` once per gRPC provider to see which features it supports (`slots`, Clock streaming, filter limits); see [architecture.md](architecture.md#provider-probes). `just txn-probe` measures how transaction writes and their statuses arrive on the provider's streams.
