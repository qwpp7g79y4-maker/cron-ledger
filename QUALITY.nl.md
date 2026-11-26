# Kwaliteitsverslag, cron-ledger 1.0.0 (Nederlands)

pep: 7

## Open punten die meetellen

| Punt | Klasse | Aftrek |
|---|---|---|
| Geen THIRD-PARTY-LICENSES.html-pagina en geen link ernaar | S | 3 |

Cijfer: 10 − 3 = 7. Andere open punten in dit bestand hebben geen klasse en tellen niet mee. Ze zijn nog niet hersteld.

Dit bestand volgt `QUALITY.md` en bevat alleen de vermelding hieronder.

### 2026-11-25 (macOS-constructor en dode code op Windows, geen codewijziging, fix blijft `9a1d68a`, tip `92687f7`)

- `src/main.rs` gecontroleerd: de constructor `RECORD` gebruikt `__DATA,__mod_init_func` op macOS en `.init_array` op Linux, en staat in `mod early` onder `cfg(any(target_os = "linux", target_os = "macos"))`. `STDOUT_WAS_CLOSED` en `stdout_is_closed` worden op elk platform door `main` gelezen, dus geen van beide is dode code op Windows. `cargo clippy --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings` en `--target x86_64-apple-darwin`: schoon, geen `allow` nodig. Geen gat aangetoond met een falende test of met clippy, dus geen codewijziging.
- `cargo fmt --check`: schoon. `cargo clippy --all-targets --locked -- -D warnings`: schoon. `cargo test --locked`: 433 geslaagd / 0 mislukt / 0 genegeerd. `cargo test --release --locked`: 431 geslaagd / 0 mislukt / 0 genegeerd.
- De macOS- en Windows-binary draaien: not run (alleen vanaf Linux gecontroleerd en gelint voor die doelen). `cargo deny`: not run. `cargo audit`: not run. Fuzz: not run.
- 2026-11-25, Rust 1.85 (de `rust-version`) op de code van `9a1d68a`, gemeten op tip `502ce35`: `cargo +1.85 fmt --check`: schoon. `cargo +1.85 clippy --all-targets --locked -- -D warnings`: schoon. `cargo +1.85 test --locked`: 433 geslaagd / 0 mislukt / 0 genegeerd. `cargo +1.85 test --release --locked`: not run. 1.85 op macOS en Windows: not run. Geen codewijziging.
- Na `9a1d68a` zijn alleen documenten gewijzigd.

### 2026-11-25 (gesloten stdout, `--output` en speciale bestanden, fix `9a1d68a`, op basis `caae92f`)

- Zes gemelde gaten tegen de binary gecontroleerd. Open en gedicht: (1) standaarduitvoer gesloten bij de start (`>&-`) eindigde met de auditcode en zonder melding, omdat Rust een gesloten descriptor 1 vóór `main` als `/dev/null` heropent. Nu exit 2 met `standard output is closed`; een constructor legt het vast vóór de runtime (Linux en macOS). `/dev/null` blijft exit 0. (2) `--output` op een named pipe verving de pipe door een gewoon bestand. Nu exit 2 (`cannot write report`) en de pipe blijft staan. Eerst rood: `tests/cli_e2e.rs::closed_stdout_at_the_start_is_exit_two_and_dev_null_stays_zero` en `output_does_not_replace_a_special_file` faalden op `caae92f` en slaagden daarna.
- Niet open: `--output` volgde een geplante symlink niet (de link wordt vervangen, het doel houdt zijn bytes). Het uitgepakte crate draait zijn eigen `cargo test --locked` (137 bestanden; `Cargo.toml` heeft `exclude`, geen `include`, dus elk testbestand zit erin). `build.rs` leest de commit met `git rev-parse --verify HEAD`; `HEAD` als bestand lezen kan alleen misgaan in een reftable-repository, en git 2.34 hier kan er geen maken, dus er is geen falende test en `build.rs` is ongewijzigd. Een bestaand gewoon bestand wordt nog steeds vervangen, zoals de README zegt.
- `cargo test --locked`: 433 geslaagd / 0 gefaald / 0 genegeerd. `cargo test --release --locked`: 431 geslaagd / 0 gefaald / 0 genegeerd (twee tests bestaan alleen in debug-builds). `cargo fmt --check`: schoon. `cargo clippy --all-targets --locked -- -D warnings`: schoon.
- `cargo deny`: not run. `cargo audit`: not run. Fuzz: not run. macOS en Windows: not run (alleen Linux).
- Na `9a1d68a` zijn alleen documenten gewijzigd: dit verslag en `KEUR-NVT.md`, `README.md`, `README.nl.md`. Voor die wijzigingen is geen nieuwe meting gedaan.
