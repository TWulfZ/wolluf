# 006 .osg format spike

Status: Done (AC8 met after the ADR 0012 rewording on 2026-09-29; AC9 not met as written, see Deviations)
Phase: F0 · Owner: TWulfZ · Date: 2026-09-28
Links: architecture §3, §4 (D2, D9, D11), §5.2, §5.3, §7, §10, §12 (F0), §13 O1; ADR 0012 (drafted by this spec); research `03-maniahub-rejudge-drills-sessions-audit.txt` L150, L169, L333, L350; `research/scripts/rejudge/rejudge.py`, `osudb.py`, `osr_wiki.md`
Siblings: 001 workspace-foundation (crates, pins, lints), 002 osu-stable-codecs (.osr header, osu!.db, scores.db decoders, `Diagnostics`), 003 store-ledger-sync (archives `.osg` bytes in the vault and sets `play.osg_sha`), 005 desktop-shell-cli (clap root of `wolluf`)

## Problem
Every play the pilot has saved since 2026-04-25 has an undocumented `.osg` file next to its `.osr` in `Data/r`. There are 4,682 of them, totalling 511 MB (003). If they hold stable's own per-judgement record, wolluf can read LN judgements straight from the file. Re-judging LN is the weakest part of the F2 plan: ScoreV1 mid-body LN mechanics are unknown, and ScoreV2 LN is at 63% parity (research 03 L145–L148, L160). Open decision O1 in architecture §13 has to be settled before F2 starts, and the F0 exit criterion is "ADR 0012 drafted from the spike". This spike is time-boxed at **3 working days**. By the end it has to say what an `.osg` contains, how reliably that maps to chart objects, and what F2 should do with it.

## Scope
- In:
  - A pure, lenient `.osg` decoder in `wolluf-source-osu` (`codec::osg`, beside 002's codecs) that decodes every record, keeps unknown bytes raw, and derives per-record judgement events.
  - `wolluf osg dump <file>`: prints one file as a table, JSON or CSV.
  - `wolluf osg survey --corpus <root>`: decodes every `.osg` under `<root>/Data/r` and checks the structural hypotheses against the paired `.osr` header, osu!.db object counts and scores.db rows, using 002's decoders.
  - A Python oracle at `research/scripts/osg/` (`osg.py` decoder, `correlate.py`). It matches `.osg` events to per-object judgements from `research/scripts/rejudge/rejudge.py`, which is not ported until F2.
  - The findings doc `docs/research/04-osg-format.md`.
  - The draft `docs/adr/0012-osg-handling.md` (Status: Proposed), with the O1 options and the decision criteria below.
- Out (non-goals):
  - Porting the re-judge engine (F2 `wolluf-judge`).
  - Any F2 `replay_decode` stage, `VersionKey`, `replay_input` or `note_obs` rows.
  - Archiving `.osg` bytes (003).
  - Any SQL, migration, IPC command or UI.
  - Encoding or writing `.osg` outside test code.
  - Accepting ADR 0012. The spike only drafts it; acceptance happens at the F0 review with the user.

## Behaviour
- `wolluf osg dump <PATH> [--format table|json|csv] [--limit N] [--events]`
  - Header line: `client_version`, `record_count`, `stride` (29 | 45), `score_system` (`v1` | `v2`, taken from the stride), file size, and diagnostics count.
  - Table and CSV columns: `idx, t_ms, d300, d100, d50, dmax, d200, dmiss, c300, c100, c50, cmax, c200, cmiss, score, max_combo, combo, hp_raw, b4, b25, b28`, plus `f0, f1` for v2. The `d*` columns are the change from the previous record.
  - `--events` prints only derived events: `t_ms, kinds[] (each judgement added in that record, in the fixed order MAX,300,200,100,50,miss), n, combo_delta, score_delta`. A record with n = 0 appears as a `score_only` event.
  - `--format json` prints a single object `{header, diagnostics[], records[] | events[]}`. `t_ms` and every count are JSON numbers, since none can exceed 2^53.
  - A decode error prints `PARSE_FAILED: <variant> <details>` to stderr and exits 2. Diagnostics (warnings) go to stderr and still exit 0.
  - The path is opened read-only. A missing file exits 2 with `NOT_FOUND`.
- `wolluf osg survey --corpus <ROOT> [--json] [--strict] [--max-files N]`
  - It lists `<ROOT>/Data/r` once, pairs `<md5>-<filetime>.osg` with the matching `.osr` (003's naming, research 03 L168), reads osu!.db and scores.db through 003's stable-read helper, and never writes anything under `<ROOT>`.
  - It prints one row per invariant (I1–I8 below) with `pass/fail/na` counts and up to 5 example file names per failure. It also prints distributions: client versions, strides, judgements per record (0/1/2/3+), and `.osr` files without an `.osg`, by date, player name and client version.
  - `--strict` exits 1 when any invariant has a failure that is not accounted for by a listed exception class (see Domain rules). Without `--strict` it always exits 0 unless IO fails.
  - Edge cases:
    - a missing `.osr` for an `.osg` → counted as `orphan_osg`, not an error;
    - a chart missing from osu!.db → I6 is `na`;
    - a non-mania `.osg` (19 in the corpus are osu!std) → decoded, reported under mode 0, excluded from I6;
    - a decode failure → counted and listed; the survey continues (§7 per-item failure policy).

## Domain rules
- **R1.** `Data/r` names are `<beatmap md5>-<FILETIME>.osr|.osg`; FILETIME = .NET ticks − 504 911 232 000 000 000 (research 03 L168; 003 Domain rules).
- **R2.** `.osr` header judgement order is 300, 100, 50, geki = MAX, katu = 200, miss (`research/scripts/rejudge/osr_wiki.md`; `rejudge.py:read_osr`).
- **R3.** Expected judgement totals (research 03 L150, L169; `rejudge.py` counts):
  - ScoreV1 judges each LN once, so total = n_objects.
  - ScoreV2 (mod bit 1<<29) judges LN head and tail separately, so total = n_objects + n_ln.
  - Known exception class `v1_split_ln`: 11 ScoreV1 replays from 2026-06-08 to 2026-07-08 (10 HR LN, 1 NM) carry split totals n_obj + n_ln (research 03 L164).
- **R4.** Failed plays are saved on client 20260924 (79 in the corpus, from 2026-04-25 on); their judgement totals are short of R3 (research 03 L153, L162).
- **R5.** Replay time accumulates all frames, including lead-in (research 03 L158, L166). `rejudge.py:read_osr` is the reference. `.osg` `t_ms` is compared against that time base, never against osrparse.
- **R6.** Rate mods: windows are `floor(base × rate)` in map time (CLAUDE.md). `.osg` times are compared in map time.
- **R7.** Parsers are lenient with a Diagnostics collector. A structurally impossible file is a hard error (§7).
  - The leading i32 of an `.osg` is the client build (seven values from 20260312 to 20260924 in six months, each equal to the paired `.osr` version). An unknown value is a **warning**, not `UNSUPPORTED_FORMAT`, because the stride check (I1) is the real format gate. This applies 002's ADR 0015 policy (header = client build; structurally valid newer files are accepted with a warning) to `.osg`; ADR 0012 records the `.osg`-specific gate.
- **R8.** `source-osu` is read-only by construction (D9). The decoder is pure over `&[u8]`. Only `wolluf-app` touches the filesystem, and only to read.
- **R9.** Real `.osg`/`.osr`/`.osu` bytes are never committed. Fixtures are synthetic, built by a test-only encoder (CLAUDE.md hard rules; §10).

### Hypotheses under test
Each line gives the hypothesis, then its preliminary evidence (see Risks), then the invariant that confirms it.

- **H0 (structure).** File = i32 client_version + i32 record_count + record_count fixed-size records. Stride is 29 when the paired `.osr` lacks ScoreV2 and 45 when it has it. → I1, I2.
- **H1 (record layout).** Byte offsets, little-endian:

  | Offset | Field |
  |---|---|
  | 0 | i32 `t_ms` |
  | 4 | u8 `b4` (always 0) |
  | 5 | 6 × u16 cumulative counts in R2 order |
  | 17 | i32 score |
  | 21 | u16 max combo so far |
  | 23 | u16 current combo |
  | 25 | u8 `b25` (0) |
  | 26 | u16 `hp_raw` (≤ 200) |
  | 28 | u8 `b28` (0 = v1, 1 = v2) |
  | 29 (v2 only) | f64 `f0`, f64 `f1` |

  → I3, I4.
- **H2 (time semantics).** `t_ms` is the map-time instant of the input that produced the judgement: the press for notes and heads, the release (or late-miss deadline) for tails. So once an event is assigned to an object, `t_ms − object_time` is stable's own offset. → C1 (correlate.py).
- **H3 (final state).** The last record's counts and score equal the `.osr` header's counts and score, and scores.db's where a row exists. → I5, I7.
- **H4 (granularity).** One record per score-changing update. A record can carry several judgements from the same frame. Judgements are never split across records. → I8, C2.
- **H5 (totals).** The sum of the final counts follows R3 against osu!.db object counts (n_obj = circles + sliders; n_ln = sliders for mania), except for R3/R4 exception classes. → I6.
- **H6 (V1 LN ticks).** Under ScoreV1, combo rises by more than the judgement delta on LN charts (hold ticks). → C3.
- **H7 (HP).** `hp_raw` is stable's 0–200 life bar and reaches 0 in failed plays. → C4. **At risk** (Risks).
- **H8 (V2 floats).** `f0` is the ScoreV2 combo-portion accumulator, whose increment ≈ 150·log2(max(combo, 2)); `f1` is unknown. → C5.
- **Negative.** A record carries no column and no object index, so per-note judgements need an assignment step (C2).

### Invariants (survey, Rust)
- **I1** (len − 8) = count × stride, stride ∈ {29, 45}.
- **I2** stride 45 ⇔ `.osr` mods has ScoreV2.
- **I3** `b28` = (stride == 45) in every record; `b4` = 0; `b25` = 0.
- **I4** within a file, `t_ms` never decreases and no count ever decreases.
- **I5** last counts and score = `.osr` header.
- **I6** Σ last counts = R3 expectation from osu!.db (mania only; `v1_split_ln` and failed plays reported as separate classes).
- **I7** last counts = scores.db row for (md5, filetime) when one exists.
- **I8** Σ judgements per record ∈ {0, 1, 2, …}, reported as a distribution. Records with 0 judgements are reported by position (first, last, middle).

### Correlations (oracle, Python, against rejudge.py)
Run per group: V1 rice NM, V1 LN (≥ 30% LN), V2 LN, and each of DT/NC, HT, HR, Mirror.
- **C1** share of events whose `t_ms` equals a re-judge press/release time in the same judgement kind.
- **C2** assignability: share of events that match exactly one object under a (time, kind) join with a ±1 ms tolerance, split into notes, heads and tails.
- **C3** combo-minus-judgement delta on V1 LN.
- **C4** minimum and last `hp_raw` for the 79 known failed plays vs passes.
- **C5** fit of `f0` increments against combo.

## Design
- **Crates and edges.** Only existing §4 edges are used: `source-osu → core`, `app → source-osu`, `cli → app`. No new internal edge, so no ADR is needed for D1. No new external dependency: every crate used is already pinned by 001 T1 (`thiserror 2.0.21`, `clap 4.6.7`, `anyhow 1.0.104`, `proptest 1.11.0`, `insta 1.48.0`, `tempfile 3.27.0`, `serde 1.0.229`, `serde_json 1.0.151`).
- **`crates/source-osu/src/codec/osg.rs`** (pure, no IO):
  - `OsgFile {client_version: i32, score_system: OsgScoreSystem, records: Vec<OsgRecord>}`.
  - `OsgRecord {t_ms: i32, counts: JudgementCounts, score: i32, max_combo: u16, combo: u16, hp_raw: u16, b4: u8, b25: u8, b28: u8, v2: Option<[f64; 2]>}`. The raw bytes are kept named and unmodelled, so the dump shows them and nothing is interpreted before the spike confirms it.
  - `decode_osg(bytes: &[u8]) -> Result<(OsgFile, Diagnostics), CodecError>`, following 002's codec convention.
  - Errors are 002's `CodecError` with `kind = FileKind::Osg`: `Truncated`, `InvalidCount` (negative count) and a new `StrideMismatch{len, count}` variant (both the `osg` kind and the variant are appended by T2; `code()` → `PARSE_FAILED`).
  - Warnings are 002 `DiagCode`s appended by T3 (stable strings, never renumbered): `osg.unknown_client_version`, `osg.flag_stride_disagree`, `osg.nonzero_reserved`, `osg.time_decreases`, `osg.count_decreases`, `osg.empty_graph`; `detail` carries the record index/offset.
  - `KNOWN_CLIENT_VERSIONS` holds the seven observed builds. It is a data table, not a threshold.
  - `JudgementCounts` (002 `codec::score_header`) and `Diagnostics` (002 `diag`) come from 002; this spec runs after 002 T13 in the source-osu lane.
  - The encoder used by tests is `testkit::encode_osg` under 002's `test-support` feature, so app and CLI tests (T5–T7) can build synthetic files.
- **`crates/source-osu/src/codec/osg/events.rs`** (pure):
  - `events(&OsgFile) -> Vec<OsgEvent {idx, t_ms, added: JudgementCounts, n: u16, combo_delta: i32, score_delta: i32}>`.
  - `final_state(&OsgFile) -> Option<(&JudgementCounts, i32)>`.
  - `check_final(&OsgFile, header_counts, header_score) -> FinalCheck {counts_eq, score_eq}`.
- **`crates/app/src/features/plays/osg.rs`** (IO, read-only):
  - `inspect(path) -> Result<OsgDump, AppError>`. `OsgDump` bundles `header + diagnostics + records + events` and derives `serde::Serialize`. It is not an IPC DTO, so it does not derive specta (D13 is untouched).
  - `survey(root, &SurveyOptions, &CancellationToken) -> Result<OsgSurveyReport, AppError>`. It reads osu!.db and scores.db with 003's stable-read helper and decodes them with 002. It lists files with 003's `replay_dir::index` and uses rayon over files (§7), collecting results in `BTreeMap` order so output is deterministic.
  - Per-file failures are collected, never propagated.
- **`apps/cli/src/cmd/osg.rs`**: a clap `Osg {Dump, Survey}` subcommand under 005's root. It is under ~10 lines per arm (D11): it parses args, calls one app function and formats the result (table/CSV by hand, JSON through serde_json).
- **`research/scripts/osg/`**:
  - `osg.py` is the decoder oracle; the Rust goldens are cross-checked against it on the corpus.
  - `correlate.py` imports `rejudge.py` and `osudb.py` and writes per-group C1–C5 metrics to a JSON file that the user passes on the command line (default: the scratch directory, never the corpus).
- **Versioned stages / param packs:** none in F0. F2's `replay_decode` stage will own an `.osg` decoder `VERSION` if ADR 0012 lands on O1a or O1c.

## Data
- user.db migration: no. `blob.kind = 'osg'` and `play.osg_sha` belong to 003.
- cache.db change: no.
- Vault or blob changes: none in this spec. ADR 0012 must answer 003's open O9 question: is `.osg` redundant (safe to skip archiving) or irreplaceable (keep it, possibly zstd at rest)? Preliminary evidence says irreplaceable. It records stable's own judgements, which re-judging does not reproduce exactly (V2 LN 63% parity).

## IPC / UI
None. CLI only.

## Acceptance criteria
- [x] AC1: Structural decode of synthetic v1 and v2 files → `codec::osg::tests::{decodes_v1_record, decodes_v2_record, keeps_reserved_bytes_raw, empty_graph_warns}` pass.
- [x] AC2: Malformed input is a hard error, never a panic → `codec::osg::tests::{rejects_truncated_header, rejects_negative_count, rejects_stride_mismatch}` and proptests `codec::osg::props::{roundtrip_encode_decode, decode_never_panics_on_arbitrary_bytes}` pass.
- [x] AC3: Unknown client version and flag/stride disagreement are warnings → `codec::osg::tests::{unknown_client_version_is_warning, flag_stride_disagree_is_warning, time_decrease_is_warning}` pass.
- [x] AC4: Event derivation → `codec::osg::events::tests::{single_judgement_per_record, merged_chord_record_has_n2, score_only_record_has_n0, final_check_matches_header, final_check_detects_mismatch}` pass.
- [x] AC5: Deterministic dump → insta goldens `osg_dump_v1_table`, `osg_dump_v2_json`, `osg_dump_events` over the synthetic fixtures pass (`cargo nextest run -p wolluf-app osg`).
- [x] AC6: CLI contract → `apps/cli/tests/osg_cli.rs::{dump_table_exits_0, dump_json_is_valid, dump_missing_file_exits_2_not_found, dump_garbage_exits_2_parse_failed, survey_synthetic_corpus_strict_passes}` pass. These tests use a tempdir synthetic corpus and `CARGO_BIN_EXE_wolluf`.
- [x] AC7: Read-only guarantee → `survey_does_not_modify_corpus` (tempdir corpus; the mtime and sha256 of every file are unchanged after the survey) passes, and 001's banned-API grep on `source-osu` stays green (`cargo xtask check-layers`).
- [x] AC8: Corpus invariants → `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run -p wolluf-app --run-ignored only osg_corpus_invariants` asserts:
  - I1 and I2 on 100% of `.osg` files;
  - I3 on 100% after reclassifying `b25 = 1` on the final record as the full-combo flag (ADR 0012 item 7), and I4 on 100%;
  - I5 on ≥ 99.5% (preliminary: 4,673 / 4,682).
  
  Every I5/I6 failure is listed by class in the report.
  Reworded at ADR 0012's acceptance (2026-09-29); it originally read "I3 and I4 on 100%".
- [ ] AC9: `wolluf osg survey --corpus "/mnt/e/Games/osu!" --json > <scratch>/osg-survey.json` runs in < 60 s on the pilot corpus. The Python oracle `python3 research/scripts/osg/osg.py --survey "/mnt/e/Games/osu!"` agrees with it on I1–I5 counts exactly.
- [x] AC10: `python3 research/scripts/osg/correlate.py --corpus "/mnt/e/Games/osu!" --out <scratch>/osg-correlate.json` reports C1–C5 for every group listed under Correlations, with n per group.
- [x] AC11: `docs/research/04-osg-format.md` exists. It has a byte-layout table, one verdict per hypothesis (confirmed / refuted / partial, with the invariant or correlation numbers), the coverage table (which plays lack an `.osg` and why, as far as known), and the remaining unknowns.
- [x] AC12: `docs/adr/0012-osg-handling.md` exists with Status: Proposed. It covers O1a / O1b / O1c, applies the decision criteria below to the measured C2 and coverage numbers, gives a recommendation, and answers the vault question from 003.
- [x] AC13: All gates in the wolluf-sdd skill §4 pass. CLAUDE.md "Domain facts" replaces "`.osg` is undocumented" with a one-line pointer to 04-osg-format.md, and "Commands" lists `wolluf osg dump` and `wolluf osg survey`.

**Decision criteria written into ADR 0012.** The thresholds are for the spike verdict only and never enter algorithm code (D17).
- **O1a** (`.osg` is the source of truth for judgements; LN re-judging is dropped) requires all of:
  - H1–H4 confirmed;
  - C2 unique assignment ≥ 99% for heads and tails on both V1 LN and V2 LN;
  - `.osg` coverage ≥ 95% of self-scope plays since 2026-04-25.
- **O1c** (hybrid): `.osg` is a per-event oracle. It validates re-judge output at event level (a much stronger parity check than totals) and supplies the judgement of every uniquely assignable LN event, tagged with high confidence. Re-judge covers the rest and plays without an `.osg`. It requires H1–H3 and C2 ≥ 90%.
- **O1b** (not usable; offset-based LN metrics with confidence tags per ADR 0011) applies if H1 or H2 is refuted or C2 < 90%.

## Risks / open questions
- **Preliminary look (2026-09-28, read-only, Python in scratch).** These observations sharpen the hypotheses above. They are not verdicts yet.
  - **Structure.**
    - The count is now 4,682 `.osg` files, not 4,666; the corpus grows with play.
    - The leading i32 is **not** always 20260924. There are seven client builds: 20260312 (16), 20260412 (1,552), 20260612 (223), 20260622 (31), 20260624 (595), 20260711 (2,068) and 20260924 (197). All 4,682 equal the paired `.osr` version.
    - The second i32 is the record count.
    - (len − 8) = count × stride holds exactly for 4,682 / 4,682 files: stride 29 for every non-V2 replay and 45 for every V2 replay.
    - `b28` = 1 exactly in the 45-byte files. `b4` is always 0; `b25` is 0 in all but one sampled record.
  - **Final state.** The last record's counts and score equal the `.osr` header in 4,673 / 4,682 files. Of the 9 failures, 8 are V2: 7 with neither counts nor score equal, 1 with only the score equal. Classify these first.
  - **Time.** On a V1 rice NM play (0012f2ce…-134350010443098880), a V2 LN play (02b893b4…-134223203794182479) and a DT play (34585edd…-134334778853511434), every sampled `t_ms` equals `rejudge.py`'s press time exactly. On V2 the tail records carry the **release** time with a 300 judgement, and DT times are in map time. This strongly supports H2.
  - **Granularity.** Across 13.85 M records: 94.99% carry 1 judgement, 4.54% carry 2, 0.38% carry 3 or more, and 0.09% carry 0. The 0-judgement record is typically the last one, a small score-only adjustment. `t_ms` and counts are monotone in 4,682 / 4,682 files.
  - **Combo fields.** In record order the field at 21 is **max combo** and the field at 23 is **current combo**. The V1 LN sample shows combo rising 5 → 8 → 10 across single judgements (supports H6).
  - **V2 floats.** `f0` increments are 150, 150, 237.74, 300, 348.3… for combo 1, 2, 3, 4, 5, which matches 150·log2(max(combo, 2)). `f1` = 0 in the samples.
- **H7 is shaky.** Only 1 of 4,682 files ends with `hp_raw` = 0, and 14 touch 0, yet research counts 79 saved failed plays. Either failed plays have no `.osg`, or bytes 26–27 are not HP. Check C4 before naming the field.
- **Coverage.** 5,030 `.osr` files vs 4,682 `.osg`. The first `.osg` is dated 2026-04-25, the same day as the first saved fail. 348 `.osr` files lack an `.osg`; 301 of them are after 2026-04-25, and 249 of those are under `TWulfZ`. Yet `Madeline` (68) and `Klinsx` (4) replays do have one. The write trigger is therefore unknown (a local score save? viewing a replay?). **Consequence:** `.osg` cannot be the only judgement source, so O1a's coverage criterion is at real risk, and O1c is the likely outcome.
- **Assignment ambiguity.** About 5% of records merge 2–3 judgements, mostly chords in one frame. They cannot be split by time alone, but a (time, kind) join to re-judge candidates should resolve most of them. C2 measures this.
- **Earlier notes were inaccurate.** Research 03 said the files start with 20260924 and have "incrementing indices". The findings doc corrects both: the value is the client build, and the "indices" are cumulative counts and combo.
- **Cross-spec.**
  - Version handling follows 002's ADR 0015 (Accepted by the user 2026-09-28).
  - 003's O9 vault-size question depends on AC12. Until ADR 0012 is accepted the vault keeps raw `.osg` bytes (user decision 2026-09-28).
  - `JudgementCounts` / `Diagnostics` are 002's (settled at the F0 review).
- **Time box.** If C2 is not measured by the end of day 3, ADR 0012 is drafted with O1b as the default and the missing measurements listed as open questions. The spike does not extend on its own; the user decides.

## Deviations (filled at close)
Closed 2026-09-28. The spike's deliverables (codec, dump/survey, findings doc, ADR 0012 Proposed) are done and every §4 gate passes. **AC8 fails as written** and **AC9's timing is only shown warm**; both are recorded below, and ADR 0012 decides what happens next.

**AC8 (corpus invariants):**
- I3 as written ("b25 = 0 in every record") fails on 36 of 4,682 files: **b25 is set on the final record only, and only on full-combo plays** (36 of 36). The reverse does not hold exactly: one full combo, `7c2471ba…-134330486805636816` (client 20260711, V1, 0 misses, combo 3007 = max), has b25 = 0. So b25 implies FC, not the other way.
- `osg_corpus_invariants` therefore asserts: I1, I2, I3_b28, I3_b4 and I4 on all 4,682 files; every I3 failure is class `b25_final_only`; every file with b25 on its final record is a full-combo play. I5 = 4,673/4,682 (99.81 %); all 9 failures are `final_short` (8 V2, 1 V1). I6: 9 `short_total` (the same files), 36 na. I7: 9 mismatch, 36 na.
- `wolluf osg survey --strict` reports 54 unexplained failures (I3 36, I5 9, I7 9) until ADR 0012's decision item 7 lands: reword AC8 to "I3 100 % after reclassifying b25-on-final", stop emitting `osg.nonzero_reserved` for b25 on the final record in `codec::osg`, and stop counting `b25_final_only` as an I3 failure in the survey. Those follow-ups wait for the ADR to be accepted.

**AC9 (survey speed and oracle agreement):** the Rust survey and the Python oracle agree exactly on every I1–I5 pass/fail/na count, including the I3 sub-rows, and on every distribution. Release run: 2.82 s wall on a warm page cache. Cold-cache runs measured 72–114 s, but while other lanes were compiling; an idle cold run was not measured, so "< 60 s" is not demonstrated for a cold cache (the time is drvfs IO in the parallel read stage).

**Findings that correct the spec's preliminary notes:**
- H4 ("a judgement is never split across records") holds only partially: 15,508 of 14.5 M units carry into the next record. The T9 sanity check reaches C1 = 100 % on its first 50 events for 2 of 3 sample files; `0012f2ce…` gives 47/50 for that reason.
- Mania zero-judgement records occur only first (15) or last (2,213); all 10,026 mid-file ones are in the 19 osu!std files. The "14 files touch HP 0" note came from osu!std (13 std, 1 mania).
- Stable writes no `.osg` for failed plays (0 of 88 saved fails), so H7 cannot be judged from fails.
- C2 is reported both as share of all units and as share of matched units per category, because the criterion is ambiguous; O1a fails and O1c passes under either reading. zstd was not measured (no binary); zlib-9 (37 %) and xz-6 (25 %) stand in for the vault answer.

**Implementation choices:**
- T1/T9: the Python oracles got stdlib unittest files (`test_osg.py`, 9 cases; `test_correlate.py`, 12 cases). `correlate.py` also reports time-only C1/C2, per-record C1 and carry counts, and groups `v2_rice_nm` and `ez`.
- T2/T3: `OsgFile.score_system` is `Option` (None for a 0-record file). All six `osg.*` DiagCodes were appended in T2. Per-record warnings are tallied into one diagnostic per kind per file, pointing at the first record and giving a count (the corpus has 13.85 M records).
- T5: dump/diagnostics/survey rendering lives in wolluf-app (`render_dump`, `render_diagnostics`, `render_survey`) so the CLI arms stay thin. The events view adds an `idx` column; CSV joins kinds with `;` and starts with a `# ` comment header.
- T6: I6 and I7 are `na` without an `.osr`, for converts, or when the chart is missing; a missing osu!.db/scores.db is reported under `sources`. A per-file read error is an I1 failure of class `read_failed`. `--max-files N` takes an ordered prefix. `sync.rs` helpers became `pub(crate)` for reuse.
- T7: the tasks.md verify filter `osg` selects only unit tests; the AC6 tests run with `cargo nextest run -p wolluf-cli --test osg_cli`. The CLI tests carry their own small `.osg`/`.osr` encoders (the CLI may not depend on source-osu). Dump errors print `PARSE_FAILED: <details>` / `NOT_FOUND: <path>` per this spec, not 005's `error[CODE]` line. `wolluf osg` opens no data dir and takes no lock; the global `--json` is a shorthand for `--format json`; the survey installs no Ctrl-C handler (read-only, default SIGINT is safe).
- T10/T11 used two extra read-only scratch scans (coverage and field ranges); the scripts stay in scratch.

**Close (T12):** CLAUDE.md points to `docs/research/04-osg-format.md` and lists `wolluf osg dump` / `wolluf osg survey`.
