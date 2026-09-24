# AGENTS.md

Solana ağında DEX'ler arası arbitraj yapan Rust binary. Rust workspace: binary'ler `apps/*`, kütüphaneler `crates/*` altında.

**Bu proje gerçek parayla canlı işlem yapar.** Yanlış bir program ID, account sırası, fee oranı veya yuvarlama yönü doğrudan para kaybıdır. Aşağıdaki doğrulama kuralları opsiyonel değildir.

## Yapı

```text
apps/turk-binary/   # bin: argüman, config, log, çıktı
crates/tb-core/     # lib: temel tipler, hatalar
```

Yeni crate: `crates/tb-<ad>/`, paket adı `tb-` önekli (`core` gibi std ile çakışan ad yasak). Her `Cargo.toml`:

```toml
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[lints]
workspace = true
```

Dependency sürümleri yalnızca kök `Cargo.toml` → `[workspace.dependencies]` içinde. Crate'lerde `foo.workspace = true`.

## Katmanlama kuralı

- `apps/*` ince kalır: argüman parse, config yükleme, log kurulumu, çıktı basma. İş mantığı yok.
- `crates/*` mantığı taşır ve uygulama kavramı bilmez: `clap`, `println!`, `std::process::exit` yok.
- Bağımlılık tek yönlü: `apps → crates`. Crate app'e bağımlı olmaz, app'ler birbirine bağımlı olmaz.
- Crate'ler arası yön de tek ve döngüsüz: `tb-format → tb-core` olur, tersi olmaz. `tb-core` hiçbir iç crate'e bağımlı değil.
- Hatalar: lib'de `thiserror` ile tipli hata, app'te `anyhow` ile sarma.
- `main.rs` 100 satırı geçiyorsa mantık yanlış yere sızmıştır, crate'e taşı.

## Bilgi kaynakları

Öncelik sırasıyla. Üst sıradaki kaynak alttakini ezer.

1. **On-chain state (RPC)**: nihai gerçek. Program deploy edilmiş mi, account gerçekte hangi owner'da, hangi boyutta.
2. **Program kaynak kodu ve IDL (GitHub, default branch)**: account layout, instruction account sırası, discriminator, fee ve fiyat matematiği, yuvarlama yönü.
3. **MCP sunucuları** (`.mcp.json`):
   - `solanaMcp`: Solana genel. Geniş konu için önce `list_sections`, sonra `get_documentation`. Dar soru veya hata mesajı için `Solana_Documentation_Search` ya da `Solana_Expert__Ask_For_Help`. On-chain program kodu yazılır veya değiştirilirse `program_autofixer` zorunludur.
   - `raydium-docs`, `meteora`, `orca-docs`: protokol dokümanı. `search_*` ile ara, `query_docs_filesystem_*` ile sayfayı oku.
4. **llms.txt**: dokümanın tam indeksi.
   - <https://docs.raydium.io/llms.txt>
   - <https://docs.meteora.ag/llms.txt>
   - <https://docs.orca.so/llms.txt>
5. **Skill'ler**: aşağıdaki "Skill'ler" bölümü. Skill metni yöntem öğretir, on-chain gerçeğin kaynağı değildir.

Modelin eğitim hafızası kaynak **değildir**. Program ID, layout, fee oranı veya matematik "hatırlanarak" yazılmaz.

### Kaynak repolar

| Protokol        | Repo                                                                                                               | Ne için                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------- |
| Raydium CLMM    | [raydium-io/raydium-clmm](https://github.com/raydium-io/raydium-clmm)                                              | program, tick math                                       |
| Raydium AMM v4  | [raydium-io/raydium-amm](https://github.com/raydium-io/raydium-amm)                                                | program                                                  |
| Raydium CPMM    | [raydium-io/raydium-cp-swap](https://github.com/raydium-io/raydium-cp-swap)                                        | program, Token-2022                                      |
| Raydium SDK     | [raydium-io/raydium-sdk-V2](https://github.com/raydium-io/raydium-sdk-V2)                                          | referans hesaplama                                       |
| Orca Whirlpools | [orca-so/whirlpools](https://github.com/orca-so/whirlpools)                                                        | program + Rust/TS SDK                                    |
| Meteora DLMM    | [MeteoraAg/dlmm-sdk](https://github.com/MeteoraAg/dlmm-sdk)                                                        | IDL (`idls/dlmm.json`), `commons/` Rust                  |
| Meteora DAMM v2 | [MeteoraAg/damm-v2](https://github.com/MeteoraAg/damm-v2), [damm-v2-sdk](https://github.com/MeteoraAg/damm-v2-sdk) | program, SDK                                             |
| Meteora DAMM v1 | [MeteoraAg/dynamic-amm-sdk](https://github.com/MeteoraAg/dynamic-amm-sdk)                                          | legacy                                                   |
| Pump.fun        | [pump-fun/pump-public-docs](https://github.com/pump-fun/pump-public-docs)                                          | IDL (`idl/`), `docs/` (bonding curve, PumpSwap, fee'ler) |

Pump.fun için MCP veya llms.txt yok. Tek resmi kaynak bu repo: `idl/*.json` ve `docs/`. Özellikle `docs/BREAKING_*.md` dosyaları, buy/sell instruction'larına yeni account eklenen breaking upgrade'leri duyurur.

### Program ID'leri

Doğrulandı: 2026-09-24. Kaynak kod veya IDL ile mainnet `getAccountInfo` karşılaştırıldı, hepsi `executable=true`, owner `BPFLoaderUpgradeab1e…`.

| Program            | Mainnet ID                                     | Kaynak                                           |
| ------------------ | ---------------------------------------------- | ------------------------------------------------ |
| Raydium CLMM       | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | `programs/amm/src/lib.rs`                        |
| Raydium AMM v4     | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | `program/src/lib.rs`                             |
| Raydium CPMM       | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | `programs/cp-swap/src/lib.rs`                    |
| Orca Whirlpool     | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`  | `programs/whirlpool/src/lib.rs`                  |
| Meteora DLMM       | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`  | `idls/dlmm.json`, `ts-client/src/dlmm/constants` |
| Meteora DAMM v2    | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`  | `programs/cp-amm/src/lib.rs`                     |
| Meteora DAMM v1    | `Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB` | `ts-client/src/amm/constants.ts`                 |
| Pump bonding curve | `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`  | `idl/pump.json`                                  |
| PumpSwap AMM       | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`  | `idl/pump_amm.json`                              |
| Pump fees          | `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ`  | `idl/pump_fees.json`                             |

Raydium repolarında `#[cfg(feature = "devnet")]` ile ayrı devnet ID'leri var. Mainnet ID'si `not(feature = "devnet")` dalındakidir.

Tüm programlar upgradeable: layout ve account listesi değişebilir. Bu tablo bir başlangıç noktasıdır, otorite değildir.

## Skill'ler

Repoda `.agents/skills/` altında kurulu, Claude Code için `.claude/skills/` symlink. Sürümleri `skills-lock.json`'da. Kurulum: `npx skills add <repo> --skill <ad>`.

İş başlamadan ilgili skill yüklenir:

| İş                                                                                 | Skill                                    |
| ---------------------------------------------------------------------------------- | ---------------------------------------- |
| Solana client, transaction kurma, RPC, PDA, Token-2022, test (LiteSVM, Surfpool)   | `solana-dev`                             |
| Genel Rust stili, yeni kod veya review                                             | `rust-best-practices`                    |
| Borrow ve lifetime hataları (E0382, E0597, E0499 …)                                | `m01-ownership`, `m03-mutability`        |
| `Arc`, `Box`, `Rc`, `Drop`, RAII                                                   | `m02-resource`, `m12-lifecycle`          |
| Generic, trait, `dyn` ve statik dispatch                                           | `m04-zero-cost`                          |
| Newtype, typestate: geçersiz durumu temsil edilemez yapma (`Lamports`, `PoolId` …) | `m05-type-driven`                        |
| `Result`, `thiserror`/`anyhow`, retry ve backoff, geçici ve kalıcı RPC hataları    | `m06-error-handling`, `m13-domain-error` |
| tokio, kanallar, websocket stream'leri, paralel quote                              | `m07-concurrency`                        |
| Pool, route, fırsat gibi domain modelleri                                          | `m09-domain`                             |
| Hot path: quote hesabı, allocation, benchmark                                      | `m10-performance`                        |
| Crate seçimi, feature flag, workspace                                              | `m11-ecosystem`                          |
| Review'da anti-pattern avı                                                         | `m15-anti-pattern`                       |
| Rename, fonksiyon taşıma, extract                                                  | `rust-refactor-helper`                   |

Skill ile bu dosya çelişirse bu dosya geçerlidir.

## Doğrulama protokolü

**Kritik bilgi**: program ID, PDA seed'leri, instruction discriminator'ı, instruction account sırası ve writable/signer bayrakları, account layout ve offset'leri, fee oranları ve fee hesaplama sırası, tick/bin/sqrt-price matematiği, yuvarlama yönü, token program (SPL Token veya Token-2022; transfer fee ve hook extension'ları), mint decimals.

Kritik bilgi kodlanmadan önce:

1. **İki bağımsız kaynak.** Biri mutlaka program kaynağı veya IDL olmalı. Diğeri RPC, MCP ya da resmi SDK.
2. **Kaynak kaydı.** Sabitin yanında `// src: <repo>@<commit-sha> <path>` bırakılır. Upgrade sonrası neyin yeniden kontrol edileceği buradan bulunur.
3. **Çelişki varsa dur.** Kaynaklar farklı söylüyorsa tahmin etme, "yakın olanı" seçme. Farkı raporla ve insan kararını bekle.
4. **Matematik birebir.** Swap quote hesabı programın kendi kodundan port edilir, formülden türetilmez. Yuvarlama yönü (floor/ceil) ve ara tip (u64, u128, U256) aynen korunur.
5. **Karşılaştırmalı test.** Her quote fonksiyonu gerçek mainnet pool state'i üzerinde (klonlanmış account'larla LiteSVM veya Surfpool) programın simülasyon çıktısıyla birebir eşleştirilerek test edilir.
6. **Upgrade takibi.** Bir programa dokunan değişiklikten önce ilgili reponun son commit'lerine ve (Pump için) `docs/BREAKING_*.md` dosyalarına bakılır.

Doğrulanamayan bilgi `TODO(verify)` olarak işaretlenir ve o kod yolu mainnet'e çıkamaz.

## Canlı işlem güvenliği

- **Anahtarlar**: private key veya keypair dosyası asla repoya, loga, hata mesajına ya da agent context'ine girmez. Keypair yolu yalnızca env değişkeninden okunur.
- **Agent'lar mainnet'e işlem göndermez**, gerçek keypair ile hiçbir komut çalıştırmaz. Mainnet işlemini yalnızca insan başlatır.
- **Varsayılan mod dry-run.** Gerçek gönderim açık bir flag ister (örn. `--live`), config'deki varsayılan asla live olmaz.
- **Önce simülasyon.** Her işlem gönderilmeden önce `simulateTransaction` çalıştırılır. Simülasyon hatası veya beklenenden düşük çıktı işlemi iptal eder.
- **Atomik arbitraj.** Tüm bacaklar tek transaction'dadır. Her swap'ta `minimum_amount_out` sıkı hesaplanır. Kâr eşiğinin altında kalan işlem zararla land etmez, fail eder.
- **Limitler.** Maksimum işlem büyüklüğü, maksimum günlük zarar ve kill switch config'dedir. Kod bu limitleri atlayamaz.
- **Aritmetik.** Fiyat ve miktar yolunda `f64` yok. Tamsayı (u64/u128) ve `checked_*` kullanılır; taşma panic değil hatadır.
- **Test sırası.** Önce localnet/LiteSVM, sonra mainnet-fork (Surfpool), en son küçük miktarla mainnet. Mainnet ilk test ortamı değildir.

## Komutlar

```sh
just check                     # cargo check --workspace --all-targets
just fmt                       # cargo fmt --all
just lint                      # fmt --check + clippy -D warnings
just test-crate tb-core        # tek crate test (nextest)
just test -p tb-core <filtre>  # isim filtresiyle test
just deny                      # cargo-deny
just ci                        # CI'daki her şey
```

## Test

- Runner `cargo-nextest` (`.config/nextest.toml`). Doc testleri için ayrıca `cargo test --doc`.
- En dar koşuyu tercih et: tek crate (`-p`) veya isim filtresi. Tüm suite yalnızca gerektiğinde.
- Test mantığın olduğu crate'te yazılır; app katmanına test yığılmaz.

## Kod kuralları

- Clippy pedantic açık, CI `-D warnings`. `unwrap()` uyarı verir: testler dışında `?` veya `expect("neden")` kullan.
- `unsafe_code` yasak.
- Yorum yalnızca "neden" için, bariz olmayan yerde. Kodu tekrar eden yorum yazma. İstisna: on-chain sabitlerin `// src:` kaynak kaydı zorunludur.
- Değişiklikten sonra `just lint` temiz olmalı.
