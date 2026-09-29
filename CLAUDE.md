# wolluf

Training companion for osu!mania **stable**. It diagnoses per-pattern weaknesses from replays, tracks skill per session, recommends chart + section + rate, and cuts practice drills. MVP is 7K (rice + LN); everything is keymode-generic (4K next). Stack: Tauri 2, Rust workspace, React 19 + Vite + TS.

## Sources of truth (read before non-trivial work)
- `docs/architecture.md`: layers, dependency rules D1–D17, storage, feedback loop, phases F0–F5. **Binding.** Change it only through an ADR.
- `docs/adr/`: accepted decisions (MADR).
- `odd/tasks/<feature-name>.md`: the one working document of an in-flight substantial feature, on its branch; removed at close (`wolluf-odd`).
- `docs/specs/`: frozen F0 record (specs 001–006 + index). No new specs; read for context.
- `docs/research/`: verified domain findings (hit windows, formats, calculators, prior art, the pilot user's data). Grep these files before searching the web.
- `research/scripts/`: Python prototypes that act as oracles for porting (osu!.db / scores.db readers, replay re-judge harness).

## Workflow
Every change request runs through the **`wolluf-odd` skill** (Organic Driven Development): small, understood changes create no documents; substantial work keeps one `odd/tasks/<feature-name>.md`, removed at close. Explain, review, audit and plan requests are read-only: no files, and a requested plan goes in the reply.

## Environment
- Rust lives in `~/.cargo/bin`. Non-interactive shells need `export PATH="$HOME/.cargo/bin:$PATH"`.
- The pilot's osu! install is at `/mnt/e/Games/osu!` (`WOLLUF_CORPUS`). **Read-only, always.** Tests and tools never write there.
- WSL2 cannot memory-read osu!, so live E2E runs on a Windows build. F0–F3 work entirely from local files.

## Hard rules (details in architecture §4)
- Domain crates (below `engine`) do no IO: no fs, net, env or `SystemTime::now`. Time, params and seeds are passed in.
- Only `wolluf-store` contains SQL. Only `app::export` may write into the osu! folder, and only with an `ExportPermit`.
- No new crate dependency edge without an ADR (`cargo xtask check-layers` enforces this).
- Thresholds and weights live in param structs, never inline. Stable string ids (axes, patterns, error codes) are never renumbered.
- Every derived artifact carries its `VersionKey`. Bump the stage `VERSION` when outputs change (stage-lock CI).
- Skill is computed only for the selected identity scope. Other players' plays never feed a self profile or telemetry.
- No GPL/LGPL in-process. Sunny is reimplemented clean-room from the PDF. MIT ports (Interlude `prelude/`, mania-hub `algorithms/`, LeoBlack) are credited in NOTICE.
- Never commit real beatmaps, audio, replays or the user's DBs. Fixtures are synthetic or minimized and anonymized.

## Domain facts that are easy to get wrong
- The 7K Regular axes are **jack, tech, speed, stream**. Stamina is derived. The LN axes are general, tech, inverse, release.
- This stable build (osu!.db 20260924) saves **failed plays** to scores.db and `Data/r` (fails appear from 2026-04 on). The `Data/r` naming is `<beatmap md5>-<FILETIME>.osr`. The `.osg` layout, invariants and verdicts are in `docs/research/04-osg-format.md` (ADR 0012, Accepted).
- The cfg `Username` can be garbage (`TWulfZasdasdasd d jSS||`), so identity matches by normalized prefix: only the session user (aliases equal to, or a prefix of, the newest cfg login, normalized length ≥ 4) is auto-selected; every other alias is listed unticked, with no suggestion (architecture §5.6, ADR 0005).
- Replay time must accumulate **all** frames, including lead-in; osrparse is wrong here. Rate-mod windows are `floor(base × rate)` in map time. Under ScoreV2, LN heads and tails are judged separately. LN judging is approximate, so tag it with a confidence.
- `osu-db` on crates.io: 0.3.0 (2021) cannot read the current osu!.db. Prefer our own codec, validated against `research/scripts/*/osudb.py` and `sdb.py`.

## Commands
Gates, from the repo root (per change, the ones `wolluf-odd` §6 marks as applicable; all of them at feature close):
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings` (002 also runs it with `--all-features`)
- `cargo xtask check-layers`
- `cargo xtask stage-lock --check`
- `cargo xtask lint-canary` (a new clippy.toml path needs a matching call in `xtask/lint-canary/src/lib.rs`)
- `cargo deny check`
- `cargo nextest run --workspace`
- `cargo xtask bindings && git diff --exit-code apps/desktop/ui/src/ipc/bindings.ts`
- `pnpm -C apps/desktop/ui exec tsc --noEmit && pnpm -C apps/desktop/ui lint && pnpm -C apps/desktop/ui test` (`pnpm -C apps/desktop/ui build` for `dist/`)

Corpus harnesses (`#[ignore]`, read-only; do not run while osu! is running, the tests fail if the corpus changes):
- All: `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run --workspace --run-ignored only`
- Codecs, with the AC15 speed budgets (release only): `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run -p wolluf-source-osu --all-features --release --run-ignored only`. `WOLLUF_PYTHON` overrides the `python3` used for the oracle.
- Sync, identity and `.osg` on the pilot: `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run -p wolluf-app --run-ignored only` (filter with `-E 'test(corpus_sync_pilot)'`, `players_corpus_selection`, `osg_corpus_invariants`)

Fixtures (deterministic; a second run must leave `git diff fixtures/` empty):
- `cargo xtask fixtures dbs --corpus "/mnt/e/Games/osu!"` (anonymized, minimized DBs)
- `cargo xtask fixtures synthetic` (synthetic `.osr`)

CLI (`cargo run -p wolluf-cli -- …`, binary `wolluf`; global `--data-dir <DIR>`, `--json`, `--log <FILTER>`):
- `wolluf setup detect`, `wolluf setup set <path>`, `wolluf setup status`
- `wolluf sync` (Ctrl-C cancels, exit 130), `wolluf players list`, `wolluf jobs list [--limit N]`
- `wolluf osg dump <file> [--format table|json|csv] [--events] [--limit N]`
- `wolluf osg survey --corpus <root> [--json] [--strict] [--max-files N]` (opens no data dir; use `--release` for timing)
- Env: `WOLLUF_OSU_DIR` (install candidate checked first), `WOLLUF_DATA_DIR` (data dir), `WOLLUF_LOG` (log filter)

Desktop:
- `cargo tauri dev`, run from `apps/desktop/src-tauri`. On WSLg a blank window needs `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
- Windows build: clone natively on NTFS (not `\\wsl$`), rustup msvc toolchain 1.98.1, VS Build Tools with the C++ workload, WebView2 runtime, Node 24 + pnpm; then `cargo tauri build --bundles nsis`. Cross-compiling from WSL is not supported.
