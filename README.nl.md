# cron-ledger

[English](README.md) · Nederlands

Een gratis, open-source, alleen-lezen opdrachtregelprogramma in Rust. Het leest crontabs en systemd-timers, berekent wanneer elke job in een tijdvenster *had moeten* draaien, legt dat naast syslog of de journal, en meldt **OK**, **LATE**, **MISSED** of **NO_TRACE**. Het draait lokaal, maakt geen netwerkverbinding en schrijft niets.

Versie **0.2.0**. Licentie: `MIT OR Apache-2.0`.

De tekstuitvoer is Engels, dezelfde taal als `--help` en de statuswoorden (`expected`, `found`, `last run`, `Summary`, `Log coverage`, `Window`, `Tolerance`, `Warnings`, `… and N more`).

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
            --since <RFC3339|30d> [--until <RFC3339|30d>] [--tz Europe/Amsterdam]
            [--tolerance 5m] [--json] [-h | --help]
```

`-` betekent stdin; over alle opties samen mag hooguit één bron van stdin lezen. `--since`/`--until` nemen een RFC3339-tijdstip of een duur in de stijl van `systemd.time(7)`, teruggerekend vanaf nu (`30d`, `2h`...). Elke andere waarde is een gebruiksfout en eindigt met exitcode 2, met de melding `'{value}' is not an RFC3339 timestamp or a duration like '30d'`. Alle voorbeelden gebruiken de fixtures onder `tests/fixtures/`, dus elk voorbeeld is ook precies wat de testsuite draait. Behalve waar een opdracht juist de standaardscan van `--system-root` laat zien, zetten de voorbeelden `--system-root /nonexistent`. Een expliciete `--crontab`, `--crontab-user`, `--timer-unit` of `--timers-list` zet die scan al uit, dus dat pad wordt niet gelezen. Het blijft in het voorbeeld staan zodat het weglaten van de bronvlag je eigen `/etc/crontab` en `/etc/cron.d/*` niet meeneemt.
De paden `tests/fixtures/` hieronder bestaan alleen in een kloon van
deze repository (`git clone`, daarna `cargo install --path .`). Na
`cargo install cron-ledger` geef je je eigen bestanden mee met
`--crontab` en `--syslog`.

### Systeemcrontab (`--crontab`, en de standaardscan van `--system-root`)

Zonder `--crontab`, `--crontab-user`, `--timer-unit` of `--timers-list` scant cron-ledger zelf `--system-root DIR` (standaard `/`) op `DIR/etc/crontab`, `DIR/etc/cron.d/*` en `DIR/var/spool/cron/**`, dezelfde indeling als een echt systeem. `--system-root` wijst die scan naar een momentopname in plaats van `/`, en de scan draait dan nog steeds. Eén van die vlaggen kiest precies de bronnen die je noemt en scant de systeemroot niet óók. Hetzelfde bestand een tweede keer lezen plant het commando twee keer in, en één logregel kan maar één van de twee kopieën dekken, zodat de extra kopie als `MISSED` zou gelden.

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

### Opgeslagen `systemctl list-timers`-tekst (`--timers-list`)

cron-ledger roept `systemctl` niet aan. Geef een tekstbestand mee dat je
bewaarde van `systemctl list-timers --all` (kopregel en de voetregel
`N timers listed.`). Alleen LAST, NEXT en UNIT worden gelezen. Er staat
geen kalender in die lijst, dus dit is alleen een controle of de laatste
run ouder is dan verwacht: als NEXT méér dan `--tolerance` vóór `--until`
ligt, en LAST ontbreekt of méér dan `--tolerance` vóór die NEXT ligt, is
de timer `MISSED` en is de laatste run in het rapport de LAST-kolom.
Anders is die laatste run `OK`. Een rij met `n/a` of `-` in beide kolommen
wordt met een waarschuwing overgeslagen. Een bestand zonder bruikbare rij
is exitcode 2. De controle gebruikt geen logs, dus een lege log maakt er
geen `NO_TRACE` van.

```sh
cron-ledger --system-root /nonexistent \
  --timers-list tests/fixtures/systemd/list-timers.txt \
  --since 2026-10-23T00:00:00Z --until 2026-10-25T00:00:00Z --tz UTC
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

Een timer-run is een manager-regel (`SYSLOG_IDENTIFIER=systemd`) waarvan `MESSAGE` begint met `Started `, of met `Finished ` wanneer die activatie nog geen `Started ` logde. `Starting ` is geen run. `UNIT` noemt de unit, of `_SYSTEMD_UNIT` als `UNIT` ontbreekt.

### `--json` voor scripts en CI

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --json
```

## Voorbeeld

In `examples/` staan twee verzonnen invoerbestanden, elk met de
verwachte uitvoer. Vanuit een kloon van deze repository:

```sh
cron-ledger --system-root /nonexistent \
  --crontab-user ops=examples/backup.crontab \
  --syslog examples/backup.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-23T00:00:00Z --tz UTC
```

De back-up draaide elke dag, maar op 21 oktober 11 minuten te laat
(`LATE`), en de taak van 12:00 draaide maar één keer (twee `MISSED`),
dus de exitcode is 1. De volledige uitvoer staat in
`examples/backup.expected.txt`; het tweede voorbeeld, `examples/sample.*`,
is een schone run die met 0 eindigt. `tests/examples_golden.rs`
vergelijkt beide met hun opgeslagen uitvoer.

## Opties

| Optie | Betekenis |
|---|---|
| `--crontab PATH\|-` | Systeemcrontab (vijf velden, een gebruiker, een commando). Herhaalbaar. `-` is stdin. |
| `--crontab-user USER=PATH` | Crontab van één gebruiker `USER` (zonder gebruikerskolom). Herhaalbaar. |
| `--system-root DIR` | Rootmap die wordt gescand op `etc/crontab`, `etc/cron.d` en `var/spool/cron` als er geen crontab- of timerbron is opgegeven. Standaard `/`. |
| `--timer-unit PATH` | Een systemd-`.timer`-unitbestand; de `OnCalendar=` wordt uitgerekend. Herhaalbaar. |
| `--timers-list PATH\|-` | Een opgeslagen `systemctl list-timers --all`-tekst. Herhaalbaar. |
| `--syslog PATH\|-` | Een opgeslagen syslog of cronlog; `.gz` wordt uitgepakt. Herhaalbaar. |
| `--journal-json PATH\|-` | Een opgeslagen `journalctl -o json`-export. Herhaalbaar. |
| `--since <RFC3339\|30d>` | Begin van het venster. Verplicht. |
| `--until <RFC3339\|30d>` | Einde van het venster (half-open). Standaard: nu. |
| `--tz ZONE` | IANA-tijdzone voor het uitrekenen en voor logregels zonder zone, bijv. `Europe/Amsterdam`. Standaard: systeemzone, anders UTC. |
| `--tolerance SPAN` | Hoe laat een gebeurtenis mag zijn en toch `OK` is. Standaard `5m`. |
| `--json` | Eén JSON-document in plaats van het tekstrapport. |
| `-h`, `--help` | Toon de gebruikstekst en stop met 0. |

`-` betekent stdin; hooguit één bron over alle opties mag stdin zijn.

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
| `2` | Gebruiks- of invoer/uitvoerfout (verkeerde opties, een `--since`/`--until`-waarde die geen RFC3339-tijdstip en geen duur is, `--until` eerder dan `--since`, meer dan één stdin-bron, een expliciet opgegeven invoerbestand dat niet te lezen is, een crontab of timer die UTF-16/UTF-32 is of een NUL-byte bevat, een log die UTF-16/UTF-32 is of waarvan minstens één byte op vier NUL is, een argument dat geen geldige UTF-8 is, of een genoemde bron die geen enkele job oplevert (leeg, alleen ongeldige regels of een timer zonder trigger of zonder geldige `OnCalendar=`; ook een lege `--crontab -`), een `--system-root` die geen map is of waarvan de scan wel crontabbestanden vond maar er geen kon lezen (een root zonder crontabbestanden is exitcode 0 met de waarschuwing `no crontab file found`), een `--since`/`--until`-venster waarvan de planning voorbij de limiet van verwachte runs uitdijt, of het rapport kan niet worden geschreven). |
| `3` | Geen `MISSED`/`LATE`, maar wel minstens één `NO_TRACE`. |

## Wat het NIET doet

- Geen live monitoring of alerting; het is een achteraf-analyse van een venster.
- Het ziet geen runs als er geen logs zijn. Dekking is één gesloten bereik per invoerbestand, van de eerste herkende regel van dat bestand tot de laatste, niet één bereik over alle bestanden. Een verwachte tijd binnen het bereik van een bestand zonder passende regel is MISSED. Eén syslog met een regel op 18 oktober 03:00 en een regel op 22 oktober 03:00 meldt 19, 20 en 21 oktober nog steeds als MISSED: die dagen liggen in dat bestand. Dezelfde twee regels in twee bestanden die niet op elkaar aansluiten, zoals `s.2.gz` dat op de 18e eindigt en `s.1` dat op de 22e begint, melden die drie dagen als NO_TRACE. Een gat tussen bestanden is geen bewijs dat de job niet draaide; geef elke rotatie apart mee als `--syslog` (een bestand dat je zelf aan elkaar hebt geplakt is één bereik). Verwachte tijden vóór de vroegste regel, ná de laatste regel, en een lege log zijn NO_TRACE. In `--json` zijn `log_coverage.first` en `log_coverage.last` de buitenste grenzen over de bestanden. Een tijd daartussen is alleen MISSED als die ook in het bereik van één enkel bestand valt. Het tekstrapport noemt het bereik van elk bestand.
- Weet niet of een job **gelukt** is, tenzij cronie CMDEND of de journal de exitcode logt.
- Geen anacron, fcron, Kubernetes CronJobs of macOS launchd (v1). Geen OnBootSec/OnUnitActiveSec.
- Argumenten moeten geldige UTF-8 zijn; een argument dat dat niet is wordt met exitcode 2 geweigerd in plaats van lossy gelezen. Een bestand onder `var/spool/cron` met een naam die geen geldige UTF-8 is heeft geen bekende gebruiker en wordt met een waarschuwing overgeslagen (het kan een niet-gecontroleerde job zijn), in plaats van als gebruiker `""` gecontroleerd te worden.
- Leest geen crontabs via `crontab -l` zelf; geef de uitvoer via stdin.
- Roept `systemctl` niet aan. `--timers-list` leest een opgeslagen lijst; het programma voert `systemctl` nooit uit.
- Het DST-gedrag volgt de cronie-daemon (`cron(8)` en `src/cron.c`): een vaste tijd in de lentesprong draait één keer, direct na de sprong; een vaste tijd in het herhaalde herfstuur draait één keer; een wildcard-uur draait in elk fysiek uur. `crontab(5)` zegt dat de sprong nooit matcht en het herhaalde uur twee keer draait; die pagina komt niet overeen met de daemon. Andere cron-implementaties kunnen afwijken. Cronie kan een job met een `CRON_TZ` anders dan de systeemzone missen of dubbel draaien; cron-ledger rekent elke job in zijn eigen zone met dezelfde regels.

"Alleen-lezen" en "offline" worden alleen geclaimd voor wat `tests/readonly_fixtures.rs` en `tests/readonly_strace.rs` echt afdekken.

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
| Een expliciete `--crontab`, `--crontab-user`, `--timer-unit` of `--timers-list` is de volledige verzameling jobs. De scan van `--system-root` draait dan niet óók, zodat een bestand onder die root niet een tweede keer wordt ingepland. `--system-root` zonder die vlaggen scant die map nog steeds | `src/app.rs::explicit_crontab_is_not_counted_again_by_the_system_root_scan`, `src/app.rs::timers_list_does_not_scan_the_system_root`, `src/app.rs::system_root_default_scan_reads_etc_crontab_cron_d_and_spool` |
| `--timers-list` leest een opgeslagen `systemctl list-timers --all`-lijst (alleen LAST, NEXT, UNIT; geen aanroep van `systemctl`). De LAST-tijd is de laatste run in het rapport. Een NEXT die al vóór `--until` verschuldigd was, voorbij `--tolerance`, en waarvan LAST ontbreekt of ouder is dan die NEXT met méér dan `--tolerance`, is `MISSED`; anders is de laatste run `OK` | `src/systemd/list_timers.rs::fixture_last_runs_are_parsed`, `tests/cli_report.rs::timers_list_fixture_puts_the_last_run_in_the_report` (`tests/fixtures/systemd/list-timers.txt`) |
| Een `journalctl -o json`-regel die een sleutel herhaalt wordt overgeslagen met een waarschuwing en is geen bewijs: er wordt geen run vastgelegd en de regel verlengt de logdekking niet | `src/log/journal.rs` (`duplicate_message_key_is_not_evidence`, `repeated_key_in_a_journal_object_is_not_evidence`) |
| Een `--since`/`--until`-venster is begrensd op 5.000.000 verwachte runs over de hele audit. Het uitklappen van één schema stopt zodra dat schema al boven de grens zit, zodat een breed venster een duidelijke exitcode 2 is in plaats van eerst elk tijdstip te alloceren. Precies 5.000.000 runs worden nog steeds geaudit | `src/app.rs::window_over_the_expected_run_cap_is_a_usage_error_exit_two`, `src/app.rs::window_at_the_expected_run_cap_still_audits`, `src/schedule.rs::expand_bounded_stops_materialising_once_over_budget`, `src/systemd/calendar.rs::expand_bounded_stops_materialising_once_over_budget` |
| Een genoemde jobbron zonder job, of een crontab of timer die UTF-16/UTF-32 is of een NUL-byte bevat, is exitcode 2; een UTF-8-BOM wordt gestript; een `--system-root` die geen map is of waarvan de scan alleen onleesbare crontabbestanden vond, is exitcode 2, een lege root waarschuwt en geeft 0; een timer met alleen ongeldige of lege `OnCalendar=` is exitcode 2, en `OnCalendar=` zonder waarde leegt de lijst zoals in systemd; een argument dat geen UTF-8 is wordt geweigerd met exitcode 2; een spoolbestand met een niet-UTF-8-naam wordt met een waarschuwing overgeslagen; stuurtekens in een pad, regeleinden inbegrepen, worden op stderr geëscapet | `tests/cli_report.rs` (`timer_unit_with_a_utf8_bom_keeps_its_timer_section`, `utf16_crontab_is_refused_with_exit_two`, `utf16_crontab_without_a_bom_is_refused_for_its_nul_bytes`, `nonexistent_system_root_is_a_usage_error_exit_two`, `crontab_with_only_invalid_lines_is_a_usage_error_exit_two`, `empty_stdin_crontab_is_a_usage_error_exit_two`, `empty_crontab_file_is_a_usage_error_exit_two`, `empty_system_root_scan_warns_and_exits_zero`, `system_root_scan_with_only_unreadable_files_is_a_usage_error_exit_two`, `system_root_with_one_unreadable_and_one_readable_file_still_audits`, `timer_with_only_an_invalid_oncalendar_is_a_usage_error_exit_two`, `timer_with_an_empty_oncalendar_is_a_usage_error_exit_two`, `timer_with_one_valid_and_one_invalid_oncalendar_still_audits`, `existing_non_utf8_file_name_is_refused_as_such`, `spool_file_with_a_non_utf8_name_is_skipped_with_a_warning`, `control_characters_in_a_path_are_escaped_in_error_messages`, `newline_in_a_path_cannot_forge_a_second_stderr_line`, `newline_in_a_scanned_file_name_cannot_forge_a_stderr_line`, `empty_report_with_no_jobs_and_empty_syslog_is_in_english`), `src/systemd/timer_unit.rs::empty_oncalendar_resets_the_list`, `src/report/mod.rs::neutralize_escapes_esc_newline_cr_and_tab` |
| Een regel met een NUL-byte in een syslog, journal of `.gz`-log wordt overgeslagen met de waarschuwing `N line(s) with a NUL byte skipped`; de rest van de log wordt nog geaudit, tenzij minstens één byte op vier NUL is (`nuls*4 >= len`) | `src/log/source.rs::nul_share_boundary_is_one_in_four_for_a_lossy_log`, `tests/cli_report.rs` (`nul_in_the_middle_of_a_syslog_skips_only_that_line`, `nul_in_the_middle_of_a_gz_syslog_skips_only_that_line`, `utf16_syslog_without_a_bom_is_still_refused_for_its_nul_bytes`) |
| Het tekstrapport toont hoogstens 50 waarschuwingen, met een regel `… and N more`; de `warnings`-array van `--json` heeft altijd de volledige lijst en het aantal | `src/report/text.rs::many_warnings_are_truncated_in_text_but_full_in_json`, `exactly_the_warning_cap_is_not_truncated` |

## Ontwikkeling

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo package
```

De eerste drie draaien in CI (`.github/workflows/ci.yml`), samen met `cargo audit` en `cargo deny check`. Draai `cargo package` zelf voor een release; CI doet dit niet.

`cargo test --no-fail-fast` gaf bij de laatste meting 371 geslaagd, 0 mislukt, 0 genegeerd (253 unittests, 118 in de integratiesuites), op commit 7c7d900.

## Steunen

cron-ledger is gratis te gebruiken. Bespaart het je tijd, dan kun je het werk steunen via [Ko-fi](https://ko-fi.com/pepai).

## Licentie

Naar keuze onder [MIT](LICENSE-MIT) of [Apache-2.0](LICENSE-APACHE).
