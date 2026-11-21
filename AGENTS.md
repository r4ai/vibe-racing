# vibe-racing — agent notes

- Rust workspace (`crates/*`). `vr` binary lives in `crates/vr-cli`.
- Design: `docs/architecture.md` (layers, invariants, flows). Build/test/quality: `docs/development.md`. When writing or restructuring docs, follow `.agents/skills/docs`.
- The race-engineer skill source of truth is `skills/race-engineer/`. It is embedded into the `vr` binary (`crates/vr-cli/src/skill.rs`) and installed with `vr skill install`; do not copy it elsewhere in the repo.
- When the user talks to you as a driver on the radio, follow `skills/race-engineer/SKILL.md`.
- Respect the invariants in `docs/architecture.md` (game-specific values stay in adapters, frontends only use `RaceApi`, JSON reports are a compatibility contract).
- Before finishing: `cargo fmt --all`, `pnpm fmt` (oxfmt for Markdown/JSON/YAML; never relax the supply-chain settings in `pnpm-workspace.yaml`), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. For test quality use `cargo llvm-cov` and `cargo mutants` (see `docs/development.md`); assert exact values at thresholds, not just signs.
- No game is needed for development: `--game synthetic` and the tests cover everything except the live Win32 mapping.
