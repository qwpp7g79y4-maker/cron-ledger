# cron-ledger

[English](README.md) · Nederlands

Een gratis, open-source, alleen-lezen opdrachtregelprogramma in Rust. Het leest crontabs en systemd-timers, berekent wanneer elke job in een tijdvenster *had moeten* draaien, legt dat naast syslog of de journal, en meldt **OK**, **LATE**, **MISSED** of **NO_TRACE**. Het draait lokaal, maakt geen netwerkverbinding en schrijft niets.

Versie **0.1.0**. Licentie: `MIT OR Apache-2.0`.

De tekstuitvoer gebruikt Nederlandse en Engelse labels door elkaar; de voorbeelden hieronder tonen die precies zoals de binary hem schrijft.

## Installatie

```sh
cargo install cron-ledger
```

Of bouw vanuit een checkout van deze repository:

```sh
cargo install --path .
```

Beide installeren de binary `cron-ledger` in `~/.cargo/bin` (of waar `cargo install` op jouw systeem normaal binaries neerzet).

## Gebruik

```
cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]
            [--timer-unit PATH]... [--timers-list PATH|-]...
            [--syslog PATH|-]... [--journal-json PATH|-]...
            --since <RFC3339|30d> [--until <RFC3339>] [--tz Europe/Amsterdam]
            [--tolerance 5m] [--json]
```

`-` betekent stdin; over alle opties samen mag hooguit één bron van stdin lezen. `--since`/`--until` nemen een RFC3339-tijdstip of een duur in de stijl van `systemd.time(7)`, teruggerekend vanaf nu (`30d`, `2h`...). Alle voorbeelden gebruiken de fixtures onder `tests/fixtures/`, dus elk voorbeeld is ook precies wat de testsuite draait. Behalve waar een opdracht juist de standaardscan van `--system-root` laat zien, zetten de voorbeelden `--system-root /nonexistent`. Een expliciete `--crontab`, `--crontab-user` of `--timer-unit` zet die scan al uit, dus dat pad wordt niet gelezen. Het blijft in het voorbeeld staan zodat het weglaten van de bronvlag je eigen `/etc/crontab` en `/etc/cron.d/*` niet meeneemt.

### Systeemcrontab (`--crontab`, en de standaardscan van `--system-root`)

Zonder `--crontab`, `--crontab-user` of `--timer-unit` scant cron-ledger zelf `--system-root DIR` (standaard `/`) op `DIR/etc/crontab`, `DIR/etc/cron.d/*` en `DIR/var/spool/cron/**`, dezelfde indeling als een echt systeem. `--system-root` wijst die scan naar een momentopname in plaats van `/`, en de scan draait dan nog steeds. Eén van die drie vlaggen kiest precies de bestanden die je noemt en scant de systeemroot niet óók. Hetzelfde bestand een tweede keer lezen plant het commando twee keer in, en één logregel kan maar één van de twee kopieën dekken, zodat de extra kopie als `MISSED` zou gelden.

```sh
cron-ledger --system-root /path/to/a/system/snapshot \
  --syslog /var/log/syslog \
  --since 7d
```

Eén bestand expliciet opgeven (werkt hetzelfde voor een snapshot of de echte `/etc/crontab`):

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### Crontab per gebruiker (`--crontab-user`)

```sh
cron-ledger --system-root /nonexistent \
  --crontab-user alice=/var/spool/cron/crontabs/alice \
  --syslog /var/log/syslog \
  --since 30d
```

### systemd-timerunits (`--timer-unit`)

```sh
cron-ledger --system-root /nonexistent \
  --timer-unit tests/fixtures/systemd/persistent-backup.timer \
  --journal-json tests/fixtures/logs/journal.jsonl \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### Debian/Ubuntu- of RHEL/cronie-syslog (`--syslog`), ook een `.gz`-rotatie

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/rhel-cron.log \
  --syslog tests/fixtures/logs/syslog.2.gz \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### Uitvoer van `journalctl -o json` (`--journal-json`)

```sh
journalctl -o json --since "7 days ago" > /tmp/journal.jsonl
cron-ledger --system-root /nonexistent \
  --crontab /etc/crontab --journal-json /tmp/journal.jsonl --since 7d
```

Elke regel is één JSON-object. Een regel die een sleutel herhaalt is geen bewijs: de regel wordt overgeslagen en als waarschuwing gemeld, op dezelfde manier als een regel die niet gelezen kan worden. Een herhaalde `MESSAGE`, `SYSLOG_IDENTIFIER`, `__REALTIME_TIMESTAMP`, `_TRANSPORT` of welke andere sleutel ook is genoeg. De regel telt niet als een run en verlengt de logdekking niet.

### `--json` voor scripts en CI

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --json
```

## Statussen

| Status | Betekenis |
|---|---|
| `OK` | Binnen de tolerantie is een passende logregel gevonden. |
| `LATE` | Er is een passende regel gevonden, maar later dan de tolerantie (bij een systemd-timer met `Persistent=true` is een run die na het opstarten is ingehaald `LATE (caught up)`, niet `MISSED`). |
| `MISSED` | Geen passende regel, terwijl de logs dit moment echt dekken: de run lijkt te ontbreken. |
| `NO_TRACE` | Geen passende regel, en de logs dekken dit moment niet: geen bewijs, maar ook geen bewijs van afwezigheid. |
| `REBOOT_ONLY` | Een `@reboot`-job: er worden geen verwachte tijden berekend, alleen de gevonden boots, ter informatie. |
| `UNSUPPORTED_TRIGGER` | Een systemd-timer met alleen een monotone trigger (`OnBootSec=`, `OnUnitActiveSec=`, ...) waarvoor v1 geen verwachte tijden kan berekenen. |

## Exitcodes

| Code | Betekenis |
|---|---|
| `0` | Alles is `OK` (`REBOOT_ONLY`/`UNSUPPORTED_TRIGGER` tellen niet mee, die zijn alleen informatief). |
| `1` | Minstens één `MISSED` of `LATE`. |
| `2` | Gebruiks- of invoer/uitvoerfout (verkeerde opties, meer dan één stdin-bron, een expliciet opgegeven invoerbestand dat niet te lezen is, een `--since`/`--until`-venster waarvan de planning voorbij de limiet van verwachte runs uitdijt, of het rapport kan niet worden geschreven). |
| `3` | Geen `MISSED`/`LATE`, maar wel minstens één `NO_TRACE`. |

## Wat het NIET doet

- Geen live monitoring of alerting; het is een achteraf-analyse van een venster.
- Het ziet geen runs als er geen logs zijn. Dekking is één gesloten bereik per invoerbestand, van de eerste herkende regel van dat bestand tot de laatste, niet één bereik over alle bestanden. Een verwachte tijd binnen het bereik van een bestand zonder passende regel is MISSED. Eén syslog met een regel op 18 oktober 03:00 en een regel op 22 oktober 03:00 meldt 19, 20 en 21 oktober nog steeds als MISSED: die dagen liggen in dat bestand. Dezelfde twee regels in twee bestanden die niet op elkaar aansluiten, zoals `s.2.gz` dat op de 18e eindigt en `s.1` dat op de 22e begint, melden die drie dagen als NO_TRACE. Een gat tussen bestanden is geen bewijs dat de job niet draaide; geef elke rotatie apart mee als `--syslog` (een bestand dat je zelf aan elkaar hebt geplakt is één bereik). Verwachte tijden vóór de vroegste regel, ná de laatste regel, en een lege log zijn NO_TRACE. In `--json` zijn `log_coverage.first` en `log_coverage.last` de buitenste grenzen over de bestanden. Een tijd daartussen is alleen MISSED als die ook in het bereik van één enkel bestand valt. Het tekstrapport noemt het bereik van elk bestand.
- Weet niet of een job **gelukt** is, tenzij cronie CMDEND of de journal de exitcode logt.
- Geen anacron, fcron, Kubernetes CronJobs of macOS launchd (v1). Geen OnBootSec/OnUnitActiveSec.
- Leest geen crontabs via `crontab -l` zelf; geef de uitvoer via stdin.
- Het DST-gedrag volgt cronie; andere cron-implementaties kunnen afwijken.

Specifiek voor deze versie: `--timers-list` (de tekstuitvoer van `systemctl list-timers --all`) wordt als optie geaccepteerd maar is nog niet geïmplementeerd; een opgegeven pad wordt met een waarschuwing genegeerd, niet gelezen. "Alleen-lezen" en "offline" worden alleen geclaimd voor wat `tests/readonly_fixtures.rs` en `tests/readonly_strace.rs` echt afdekken.

## Alternatieven

- **Cronitor**, **healthchecks.io** (ook self-hostbaar, open source), **Dead Man's Snitch**: heartbeat-monitoring. Dit is beter voor live alerts, maar elke job moet vooraf een ping krijgen. Cronitor schrijft zelf dat cron-logs niet tonen welke jobs níet draaiden: https://cronitor.io/guides/where-are-cron-logs-stored
- `systemctl list-timers`: laatste en volgende run voor timers, niet voor crontabs.
- `cronic`/`chronic`: mail bij een fout, niet bij een ontbrekende run.
- Advies: gebruik cron-ledger om achteraf gaten te vinden, en een heartbeat-dienst voor alerting vooruit.

## Claim → test/fixture

Elke claim in deze README wordt gedekt door een bestaande test of fixture in deze repo. De volledige tabel, met per claim de test, staat in de [Engelse README](README.md#claim--testfixture).

| Claim | Test / fixture |
|---|---|
| `MISSED` alleen binnen het bereik van één invoerbestand, van de eerste herkende regel van dat bestand tot de laatste. Een gat binnen dat bereik zonder passende regel is `MISSED`, ook 19–21 oktober tussen een regel op de 18e en een regel op de 22e in hetzelfde bestand. Een gat tussen twee bestanden, ook niet-aansluitende `s.1` en `s.2.gz`, een gat vóór het eerste bestand of ná het laatste, een lege log, en een gat dat precies op de venstergrens tussen twee bestanden valt, zijn `NO_TRACE`. Een tijd gelijk aan de eerste of laatste regel van een bestand valt binnen dat bereik. `--json` `log_coverage` is de buitenste grens; het tekstrapport noemt elk bestandsbereik | `tests/ledger_golden.rs` (`log_gap_missed_only_in_covered_days`, `interior_gap_between_first_and_last_log_line_is_missed`, `gap_between_two_log_files_is_no_trace`, `gap_on_the_window_edge_between_two_files_is_no_trace`), `src/app.rs::non_adjacent_rotation_files_are_separate_coverage_spans`, `tests/cli_report.rs::no_trace_only_scenario_exits_three` (`tests/fixtures/report/no_trace/`) |
| Een expliciete `--crontab`, `--crontab-user` of `--timer-unit` is de volledige verzameling jobs. De scan van `--system-root` draait dan niet óók, zodat een bestand onder die root niet een tweede keer wordt ingepland. `--system-root` zonder die vlaggen scant die map nog steeds | `src/app.rs::explicit_crontab_is_not_counted_again_by_the_system_root_scan`, `src/app.rs::system_root_default_scan_reads_etc_crontab_cron_d_and_spool` |
| Een `journalctl -o json`-regel die een sleutel herhaalt wordt overgeslagen met een waarschuwing en is geen bewijs: er wordt geen run vastgelegd en de regel verlengt de logdekking niet | `src/log/journal.rs` (`duplicate_message_key_is_not_evidence`, `repeated_key_in_a_journal_object_is_not_evidence`) |
| Een `--since`/`--until`-venster is begrensd op 5.000.000 verwachte runs over de hele audit; daarboven een duidelijke exitcode 2 in plaats van onbegrensde geheugengroei | `src/app.rs::window_over_the_expected_run_cap_is_a_usage_error_exit_two`, `src/app.rs::window_at_the_expected_run_cap_still_audits` |
| Het tekstrapport toont hoogstens 50 waarschuwingen, met een regel `… en N meer`; de `warnings`-array van `--json` heeft altijd de volledige lijst en het aantal | `src/report/text.rs::many_warnings_are_truncated_in_text_but_full_in_json`, `exactly_the_warning_cap_is_not_truncated` |

## Ontwikkeling

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo package
```

De eerste drie draaien in CI (`.github/workflows/ci.yml`), samen met `cargo audit` en `cargo deny check`. Draai `cargo package` zelf voor een release; CI doet dit niet.

## Steunen

cron-ledger is gratis te gebruiken. Bespaart het je tijd, dan kun je het werk steunen via [Ko-fi](https://ko-fi.com/pepai).

## Licentie

Naar keuze onder [MIT](LICENSE-MIT) of [Apache-2.0](LICENSE-APACHE).
