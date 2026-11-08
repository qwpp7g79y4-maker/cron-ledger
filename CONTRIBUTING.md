# Contributing

Thanks for your interest in cron-ledger.

## Building and testing

You need Rust 1.85 or newer. The `readonly_strace` test uses `strace`; install it to run that test (CI installs it).

```sh
cargo build
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked --no-fail-fast
```

These checks must pass before a change is merged, on stable and on Rust 1.85 (`cargo +1.85 clippy ...`, `cargo +1.85 test ...`). Test fixtures must be synthetic: never add a real log or a real crontab.

## Issues

Open an issue on the [issue tracker](https://github.com/qwpp7g79y4-maker/cron-ledger/issues) for bugs and feature requests. Never paste a real log or crontab: it can hold secrets in plain text. Report security problems privately as described in [SECURITY.md](SECURITY.md).

## Commit messages

Commits follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) in English, for example `fix: refuse a FIFO as log file` or `docs: clarify the missed-run window`.

## Contact

PEPAI, pep_ai@icloud.com
