# AGENTS.md

Rust workspace. Binary'ler `apps/*`, kütüphaneler `crates/*` altında.

## Yapı

```
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
- Yorum yalnızca "neden" için, bariz olmayan yerde. Kodu tekrar eden yorum yazma.
- Değişiklikten sonra `just lint` temiz olmalı.
