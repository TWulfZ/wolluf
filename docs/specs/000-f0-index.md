# 000 F0 Base: index

Status: In progress (close stage ran 2026-09-28; open items under "Close status")
No new specs are added here. F0's open "Close status" items are resolved as ordinary `wolluf-odd` work and ticked here; everything after F0 follows `wolluf-odd`.
Phase: F0 · Owner: twulfz · Date: 2026-09-28
Links: architecture §12 (F0 row), §4 (D1–D17), §13 (O1, O9, O10); `.claude/skills/wolluf-sdd/SKILL.md`

## Goal
Take wolluf from docs-only to a desktop app and a `wolluf` CLI that detect the pilot's osu! stable install, ingest its plays into a ledger wolluf owns, and do it idempotently. They archive the original replay, `.osg` and chart bytes, and show the "Which of these are you?" list with correct defaults. The architecture guards (layer checks, lints, licence gate, CI on Windows and Linux) are enforced from the first commit, and the `.osg` spike settles O1 enough to draft ADR 0012. Nothing in F0 computes skill; F0 builds the raw-data and identity base that F1–F3 derive from.

## Specs

| Spec | Title | Status | Crates / dirs written | ADRs |
|---|---|---|---|---|
| [001](001-workspace-foundation/spec.md) | Workspace foundation | In progress | root, `crates/core`, `xtask`, CI, `docs/adr` 0001–0009 | 0001–0009 (Accepted) |
| [002](002-osu-stable-codecs/spec.md) | osu! stable codecs | In progress | `crates/source-osu` (codecs, install, probe), `fixtures/dbs`, `research/scripts/oracle` | 0015 (Accepted) |
| [003](003-store-ledger-sync/spec.md) | Store, play ledger and SyncPlays | Done | `crates/store`, `crates/source-osu` (snapshot, replay_dir, songs, watch), `crates/app` (context, jobs, plays) | 0014 (Proposed) |
| [004](004-players-identity/spec.md) | Players and identity | Done | `crates/core` (`ScopeHash`), `crates/store` (players repos), `crates/app` (players), desktop `commands/players.rs`, UI `features/players` | 0005 amendment |
| [005](005-desktop-shell-cli/spec.md) | Desktop shell and CLI | In progress | `crates/app` (errors, clock, logging, setup), `apps/desktop/src-tauri`, `apps/desktop/ui`, `apps/cli`, CI additions | 0009 (verifies) |
| [006](006-osg-spike/spec.md) | .osg format spike | Done | `crates/source-osu` (`codec::osg`), `crates/app` (plays::osg), `apps/cli` (`osg`), `research/scripts/osg`, `docs/research/04` | 0012 (Proposed) |

ADR numbers are final: 0010, 0011 and 0013 stay reserved for F1–F3 (§11).

### Single owners (settled at the F0 review)
- `PlayId`, `DotNetTicks`, `FileTime`, `BlobSha256`, `VersionKey`, `ErrorCode`, `Clock`: 001 (core). `PlayId` hashes `FileTime` as `i64` LE; ADR 0006 freezes the encoding.
- Every F0 dependency pin: 001 T1. Sibling tasks only write `x.workspace = true`.
- Install discovery and validation (env, `.osz` registry handler, candidate scan, lazer detection): 002 T12. 005 maps it to `setup_*`.
- `Data/r` name parsing: 002 `ReplayFileName::parse`. 003's `replay_dir` calls it.
- `JudgementCounts`, `Diagnostics`, `CodecError`, `FileKind`: 002. 006 appends `osg` entries.
- `AppError`, `IpcError`, `SystemClock`, logging: 005 T3–T4. 003 and 004 add `From` impls and message keys.
- `AppPaths` (with CLI override and `logs_dir`), `AppContext`, `register_install`, `JobRunner` (with follow-ups), job DTOs (camelCase): 003.
- user.db `0001_init`, including both `profile` partial indexes and `play.origin` (`scores_db | replay_only`): 003. cache `alias_stats` DDL: 004 T6 (cache v1 is unreleased, so there is no version bump).
- ADR 0009, including 005's shell deviations: 001 T24. Architecture edits: §3/§5.3/§7 snapshot and ledger wording in 003 T1, §5.6, the §8 identity-UX line and CLAUDE.md's identity line in 004 T1, the §7 unknown-version sentence in 002 T17, and §3 plugins, §8 and the CLI name in 005 T20.

## Global execution order

Lanes are the disjoint write sets that agents may work in at the same time. A lane has one owner at a time. Tasks inside a lane run in the order listed. Root `Cargo.toml` belongs to the orchestrator.

| Lane | Write set |
|---|---|
| ROOT | `Cargo.toml`, toolchain/config files |
| CORE | `crates/core` |
| XTASK | `xtask/`, `deny.toml` (later `fixtures/`) |
| SRC | `crates/source-osu` |
| STORE | `crates/store`, `fixtures/userdb` |
| APP | `crates/app` |
| DESK | `apps/desktop/src-tauri` |
| CLI | `apps/cli` |
| UI | `apps/desktop/ui` |
| PY | `research/scripts/{oracle,osg}` |
| ADR | `docs/adr/*` (one file per task, so ADR tasks may also run in parallel with each other) |
| ARCH | `docs/architecture.md`, `CLAUDE.md`, `.github/` (serial) |

### Stage 0: skeleton (serial)
1. **001-T1** (ROOT, orchestrator): workspace, the full F0 pin set, stub crates.

### Stage 1: foundation (parallel groups)
- **G1 CORE:** 001-T2 → T3 → T4 → T5 → T6 → T7 → T8 → **004-T2** (`ScopeHash`)
- **G2 XTASK:** 001-T9 → T10 → T11 → T12 → T13
- **G3 ADR:** 001-T16 … T24 and 002-T1 in parallel; then 005-T1 (after 001-T24)
- **G4 ARCH (serial):** 001-T15 (LICENSE, NOTICE, conventions) → 003-T1 (ADR 0014 + §3/§5.3/§7) → 004-T1 (after 001-T20; ADR 0005 amendment + §5.6/§8 + CLAUDE.md)
- **G5 PY:** 002-T14, 006-T1, 006-T9 in parallel. They need only the corpus, which is how the `.osg` evidence gets gathered from day one.
- **G6 UI:** 005-T13 (scaffold; needs only Node/pnpm)

### Stage 2: CI and manifests
- ARCH: **001-T14** (ci.yml), after all of G2 (CI calls lint-canary and deny)
- Crate manifests, serial, after G1 and 001-T9/T10: **002-T2 → 003-T2** (both edit `crates/source-osu/Cargo.toml`)

### Stage 3: adapters and pure app code (parallel groups, after Stage 2)
- **G7 SRC:** 002-T3 → T4 → T5 → T6 → T7 → T8 → T9 → T10 → T12 → T13 → 003-T4 → 003-T5 → 003-T12 → 006-T2 → 006-T3 → 006-T4 (002-T11 was removed)
- **G8 STORE:** 003-T6 → T7 → T8 → T9 → T10 → T11 → 004-T6
- **G9 APP (pure):** 005-T3 (`AppError`) → 005-T4 → 004-T3 → 004-T4 → 004-T5 → 004-T7

### Stage 4: application wiring (after G7 through 003-T12, G8, G9)
- **APP (serial):** 003-T13 → T14 → T15 → T16 → T17 → T18 → T19 → 004-T8 → 004-T9 → 004-T10 → 005-T5
- In parallel with APP:
  - DESK: 005-T9 (needs 003-T13 only)
  - SRC: 002-T15 (corpus harness, needs 002-T14)
  - XTASK: 002-T16 (fixtures)

### Stage 5: shells (after 005-T5)
- DESK: 005-T10 → T11 → T12
- CLI: 005-T6 → T7 → T8
- APP: 006-T5 → 006-T6

These three run in parallel. Then:
- **004-T11** (APP + DESK + `bindings.ts`, exclusive): after 005-T12 and 006-T6
- CLI: 006-T7 (after 005-T8 and 006-T6)
- UI (serial): 005-T14 → T15 → T16 → T17 → 004-T12 → 004-T13 → 005-T18 (005-T14 starts after 004-T11, so `bindings.ts` already contains `players_*`)

### Stage 6: corpus runs, spike docs, CI additions
- APP (serial, `#[ignore]` corpus tests): 003-T20 → 004-T14 → 006-T8
- Docs: 006-T10 → 006-T11 (need 006-T8 and 006-T9)
- ARCH: 005-T19 (desktop-windows job; Node/pnpm pins in the `ui` job)
- Manual: 005-T20 smokes (WSL and Windows)

### Stage 7: close (serial, ARCH)
001-T25 (can close as soon as Stage 2 is green) · 002-T17 · 003-T21 · 004-T15 · 005-T20 · 006-T12 → F0 exit review with the user.

Critical path: 001-T1 → CORE → manifests → SRC (G7) → APP wiring (Stage 4) → shells and 004-T11 → UI → corpus → close. SRC and APP are the long single-crate chains. The only ways to shorten the path are splitting crates, which the §3 crate budget forbids, or running `[P]` work in other lanes.

The 3-day `.osg` spike runs on two tracks. Its evidence (006-T1, T9) is gathered in Stage 1. Its Rust surface (T2–T8) waits for 002, 003 and 005 to exist. The 3-day budget is spent on T1, T9, T10 and T11, not on waiting.

## F0 exit criteria (architecture §12)

| §12 criterion | Proven by |
|---|---|
| The app detects osu! | 002 AC14 `detect_finds_corpus_install`; 005 AC17 (WSL smoke) and AC18 (Windows smoke, `registry` source) |
| It ingests 4.3k plays idempotently. **Re-ingest adds 0 rows** | 003 AC11 `sync_twice_adds_zero_rows`; 003 AC17 `corpus_sync_pilot` (≥ 4,338 plays = distinct scores.db keys + replay-only imports; second sync `plays_new = 0`, `plays_replay_only = 0`); 005 AC16 `second_sync_adds_zero` |
| It archives replays and charts | 003 AC7, AC10, AC13; AC17 `replays_linked` == independent `Data/r` count |
| It shows "Which of these are you?" with correct defaults (`TWulfZ` auto; all others listed unchecked, with no suggestion; the cfg-string alias `TWulfZasdasdasd d jSS\|\|` is also auto because it equals the login) | 004 AC3 `pilot_like_selection`, AC14 (wizard UI), AC15 `players_corpus_selection`; 005 AC12 (first-run guard) |
| **Identity table tests pass** | 004 AC1–AC12 |
| **ADR 0012 drafted from the spike** | 006 AC11 (`docs/research/04-osg-format.md`), AC12 (ADR 0012, Status Proposed) |
| Workspace, xtask, deny, CI skeleton, ADRs 0001–0009, MIT `LICENSE` | 001 AC1–AC17 |
| Players feature complete (alias stats, session-user auto-selection, decisions, profiles, Select all, Merged/Compare) | 004 AC7–AC14 |
| All gates green | the `wolluf-sdd` §4 gate block on the final commit, including the corpus run |

## Close status (2026-09-28)

Gate block (wolluf-sdd §4) on the final commit: fmt, clippy `-D warnings` (with and without `--all-features`), check-layers (8 members, 0 violations), stage-lock --check (0 stages), lint-canary (58 lints), `cargo nextest run --workspace` 434 passed / 14 ignored, bindings drift clean, UI tsc + lint + vitest (126 tests), and the corpus run `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run --workspace --run-ignored only` 14/14 all pass. **`cargo deny check` fails on advisories** (two "unmaintained" crates via specta rc.25 and tauri's gtk macros; see 001 Deviations).

| §12 criterion | State |
|---|---|
| Detects osu! | `detect_finds_corpus_install` green; WSL smoke (005 AC17) passed 2026-09-29 (user); Windows smoke (AC18) pending, manual |
| Ingests idempotently, re-ingest adds 0 | `corpus_sync_pilot` green: 5,011 plays (4,969 + 42 replay-only), second sync `plays_new = 0` |
| Archives replays and charts | `replays_linked` 4,969 = independent `Data/r` count; 1,425 charts archived |
| "Which of these are you?" defaults | `players_corpus_selection` green (`TWulfZ` and the cfg-string alias auto; 8 others unticked); wizard UI tests green |
| Identity table tests | 004 AC1–AC12 green |
| ADR 0012 drafted | `docs/adr/0012-osg-handling.md`, Accepted 2026-09-29 (O1c hybrid) |
| Workspace, xtask, deny, CI, ADRs, LICENSE | Done except the first green PR run |
| All gates green | All local gates green as of 2026-09-29 (deny included) |

Decisions for the F0 exit review:
1. `cargo deny` advisories: accept targeted, reasoned ignores for RUSTSEC-2024-0436 (`paste`) and RUSTSEC-2024-0370 (`proc-macro-error`), or another policy. The close stage did not change `deny.toml`.
2. Accept ADR 0014 (Proposed) and ADR 0012 (Proposed, O1c). Accepting 0012 triggers its item 7: reword 006 AC8 around b25 as a final-record FC flag, stop warning on it in `codec::osg` and the survey, and update architecture §13 O1/O9.
3. Spec inconsistency: ingest skips mode ≠ 3, so 004's `non_mania` bucket is always 0 and 003's `skipped_non_mania` counts std plays twice on a first sync.
4. Push the branch for the first CI run (001 AC15, 005 T19), then run the 005 T20 manual checklist.

Resolution (2026-09-29, `wolluf-odd` work on `chore/f0-close`):
1. Done: targeted ignores for RUSTSEC-2024-0436 and RUSTSEC-2024-0370 in `deny.toml`; `cargo deny check` is green.
2. Done: ADR 0014 and ADR 0012 are Accepted. ADR 0012 item 7 is applied: final-record b25 is the FC flag, `--strict` unexplained failures went from 54 to 18 (the 9 `final_short` files), and architecture §13 O1/O9 are closed.
3. Done: each non-mania play is counted once per sync (pilot: 19 on the first and second sync); the always-empty `non_mania` bucket is removed.
4. Open: the first CI run on a pushed branch (001 AC15, 005 T19) and the Windows smoke (005 AC18). The WSL smoke passed.

## Known F0 deviations from the architecture (each recorded in the named ADR at close)
- Source snapshots are in-memory reads rather than temp copies, `play.passed` is nullable, and orphan `Data/r` replays become plays with `play.origin = 'replay_only'` (ADR 0014).
- Newer-than-verified DB headers are accepted after full structural validation, with a warning (ADR 0015, Accepted).
- §5.6 scored identity heuristics and tiers are replaced by the session-user rule: auto-select only the alias(es) matching the newest cfg login, list the rest unticked (ADR 0005 amendment).
- The opener plugin replaces the shell plugin, and a new `app_open_logs_dir` command is added (ADR 0009).
- `wolluf-chart` is created as an empty crate; chart types go to the F1 spec (ADR 0004).
- F0 stages `catalog` and `players.alias_stats` carry `VersionKey`s, but they join `stage_versions.lock` only when the engine registry arrives (F1).

## User decisions (2026-09-28)
- Licence: MIT, copyright 2026 TWulfZ; `publish = false` until the first release (001 T1, T15, T23).
- Newer-than-verified osu! DB versions: accepted after full structural validation, with a warning; ADR 0015 Accepted (002).
- Orphan replays (`.osr` in `Data/r` with no scores.db row): imported as plays with `origin = 'replay_only'` from the `.osr` header, same natural key (003).
- Identity: auto-select only the current session user (alias equal to, or a prefix of, the newest cfg `Username` after normalization, length ≥ 4; F3 adds the linked account). No match → nothing preselected and the wizard asks. Every other alias is listed unticked, no suggestions, sorted by play count; decisions persist and always win (004 R3–R7).
- The reviewer's vetoable calls stand: LZMA payload decoding in F2, the Songs scanner in F1, `PlayId` over `FileTime`, `osu!.exe` + `osu!.db` required for a valid install, registry discovery, an empty `wolluf-chart` in F0.
- `.osg` vault policy: keep raw bytes until the 006 spike's ADR 0012 is accepted (O9).

## Open questions
None block F0. O9 (vault `.osg` policy) is settled by ADR 0012 at the end of the 006 spike.
