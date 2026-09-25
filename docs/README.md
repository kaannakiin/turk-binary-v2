# turk-binary docs

turk-binary watches Solana DEX pools and (later) trades price gaps between them.

| Page                                 | What it covers                                                    |
| ------------------------------------ | ----------------------------------------------------------------- |
| [architecture.md](architecture.md)   | How the crates fit together and how data flows                    |
| [configuration.md](configuration.md) | Every config key and environment variable                         |
| [dexes.md](dexes.md)                 | Supported DEXes, their names in config, and how each was verified |

## Quick start

```sh
cp config.example.toml config.toml
cp .env.example .env      # fill in TB_RPC_URL, TB_GRPC_URL, TB_GRPC_X_TOKEN
just watch
```

`just` loads `.env` by itself, so the endpoints never need to be exported in your shell.

`watch` is read-only. It never signs or sends a transaction.

Run `just probe` once per gRPC provider to see which features it supports (`slots`, Clock streaming, replay, filter limits); see [architecture.md](architecture.md#provider-probes). `just txn-probe` measures how transaction writes and their statuses arrive on the provider's streams.
