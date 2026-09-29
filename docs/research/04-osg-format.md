# 04 `.osg` format: spike findings

Status: verified on the pilot corpus, 2026-09-28 (read-only). Spec: `docs/specs/006-osg-spike/spec.md`. Decision: `docs/adr/0012-osg-handling.md` (Accepted 2026-09-29).

## Summary
- An `.osg` is stable's own **judgement timeline** for one play. It has an 8-byte header (client build, record count), then one fixed-size record per score-changing update. Each record holds the map-time instant, the cumulative judgement counts in `.osr` order, score, max combo, current combo, a life value and a score-system flag. ScoreV2 files add two `f64` per record.
- Structure is exact on 4,682 / 4,682 files. The final state equals the `.osr` header on 4,673 / 4,682 (99.81%). Every record time equals re-judge's input instant for 97.1% of judgement units (98.3% counting expiry misses). The residual is bounded by the re-judge oracle, which reproduces stable's totals exactly on only 4 of 1,532 V2 LN plays.
- A record has **no column and no object index**. Per-object judgements need an assignment step. A (time, kind) join assigns 95.9% of units uniquely (heads 98.0%, tails 98.7% of matched units). That is above O1c's 90% bar and below O1a's 99% bar.
- Coverage is incomplete. Stable never writes an `.osg` for a failed play, and it also skips 4.1% of complete passes (about 6% in self scope) for an unknown reason. Self-scope coverage since the first `.osg` is 91.8%.
- Verdict for ADR 0012: **O1c (hybrid)**. Keep the raw bytes in the vault.

## Sources and method
All numbers below come from one of these outputs. Each is regenerable, and none is committed; they live in the session scratch directory.

| Tag | Command | Output |
|---|---|---|
| **S-rs** | `cargo run --release -p wolluf-cli -- osg survey --corpus "/mnt/e/Games/osu!" --json` | `osg-survey.json` |
| **S-py** | `python3 research/scripts/osg/osg.py --survey "/mnt/e/Games/osu!"` | `osg-survey-py-stage6.json` |
| **T8** | `WOLLUF_CORPUS=... cargo nextest run -p wolluf-app --run-ignored only osg_corpus_invariants` | `osg_corpus.log` |
| **C** | `python3 research/scripts/osg/correlate.py --corpus "/mnt/e/Games/osu!" --out ...` | `osg-correlate.json` (fields `groups.<g>.C1..C5`, `failed_without_osg`, `sanity`) |
| **X-cov** | read-only scratch scan: every `Data/r` `.osr` classified by `.osg` presence, date, player and completeness, using `osg.read_osr_header` and `correlate._chart_counts` | stdout (quoted below) |
| **X-fld** | read-only scratch scan of every `.osg`: `hp_raw` range, mid-file zero-judgement records by mode, compressibility of a seeded 200-file sample | stdout (quoted below) |

- **Corpus state.** 4,682 `.osg`, 5,030 `.osr`, 13,854,500 records, 510,782,980 bytes of `.osg`. The first `.osg` is dated 2026-04-25T02:40:53Z. There are 0 orphan `.osg` files and 0 unrecognised names.
- **Oracle agreement (spec AC9).** S-rs and S-py agree exactly on every I1–I5 row, including the sub-rows I3_b28, I3_b4 and I3_b25. They also agree on every distribution they share and on the nine I5 failure records.

## Byte layout
The file is `i32 client_version`, then `i32 record_count`, then `record_count` records. All values are little-endian. The stride is 29 bytes (ScoreV1) or 45 bytes (ScoreV2).

| Offset | Type | Field | Meaning | Status |
|---|---|---|---|---|
| 0 | i32 | `t_ms` | Map-time instant of the update: the press for notes and heads, the release or expiry for tails and misses; rate mods apply | confirmed (H2) |
| 4 | u8 | `b4` | always 0 | observed, meaning unknown |
| 5 | 6 × u16 | counts | cumulative 300, 100, 50, MAX, 200, miss (`.osr` order, R2) | confirmed (I4, I5) |
| 17 | i32 | `score` | cumulative score | confirmed (I5) |
| 21 | u16 | `max_combo` | max combo so far | confirmed |
| 23 | u16 | `combo` | current combo | confirmed (C3) |
| 25 | u8 | `b25` | 1 on the **final record** of full-combo plays, 0 everywhere else | corrected: not reserved (H1) |
| 26 | u16 | `hp_raw` | 0–200; behaves like the life bar | provisional (H7) |
| 28 | u8 | `b28` | 1 exactly in ScoreV2 (45-byte) files | confirmed (I3_b28) |
| 29 | f64 | `f0` (V2 only) | ScoreV2 combo-portion accumulator | confirmed as an accumulator; formula partial (H8) |
| 37 | f64 | `f1` (V2 only) | always 0 | unknown |

The file contains no player name, no chart identity beyond the file name, and no column data.

## Hypothesis verdicts

| Hypothesis | Verdict | Evidence |
|---|---|---|
| **H0** structure | **confirmed** | I1 4,682 / 4,682: (len − 8) = count × stride. I2 4,682 / 4,682: stride 45 ⇔ ScoreV2 (S-rs, S-py). Strides: 29 = 2,579 files, 45 = 2,103. The header i32 equals the paired `.osr` version in 4,682 / 4,682 files. |
| **H1** record layout | **confirmed, with one correction** | I3_b28 4,682 / 4,682, I3_b4 4,682 / 4,682, I4 4,682 / 4,682. **b25 is not reserved.** I3_b25 fails on 36 files, all of class `final_only` (S-rs `invariants[I3_b25].classes`). T8: 37 full-combo plays have an `.osg`; b25 is set on 36 of them and on nothing else. The exception is `7c2471ba…-134330486805636816` (V1, client 20260711). b25 = 1 matches the `.osr` `perfect` byte on sampled files. So b25 = 1 implies FC, but FC does not always imply b25 = 1. |
| **H2** time semantics | **confirmed** | C `all_mania.C1.unit_share` 0.971: exact (time, kind) match to a re-judge input instant. Including expiry-deadline misses: 0.983 (`C1_incl_deadline_misses`). Time only: 0.977. v1_rice_nm reaches 0.986 (0.997 with deadlines). V1 LN and V2 tails carry the release instant: tails are unique for 0.990 (V1 LN) and 0.987 (V2 LN) of matched units. Sanity (`C.sanity`): the V2 LN sample `02b893b4…` and the DT sample `34585edd…` give 50 / 50 on their first 50 events; the V1 rice sample `0012f2ce…` gives 47 / 50 (see H4). DT/NC C1 is 0.939 and EZ is 0.869 (n = 2), which reflects weak oracle mod handling (research 03 L163 EZ, L166 V2 + rate mods). |
| **H3** final state | **confirmed, with one exception class** | I5 4,673 / 4,682 = 99.81%. I7 4,637 pass, 9 fail, 36 na (no scores.db row). The 9 I5, I6 and I7 failures are the same files, class `final_short`: the `.osg` stops 1–2 judgements short while `.osr` and scores.db agree with each other (table below). |
| **H4** granularity | **confirmed, with a small carry residual** | Judgements per record (S-rs): 0 = 12,254 (0.09%), 1 = 13,160,580 (94.99%), 2 = 629,235 (4.54%), 3+ = 52,431 (0.38%). Zero-judgement records: first 15, middle 10,026, last 2,213. X-fld: **all 10,026 mid-file zero-judgement records are in the 19 osu!std files**, so no mania file has one. Carry: 15,508 units (0.107%) show up one record after re-judge's instant (`C1.units_carried_from_previous_record`), and 21,373 units are resolved by the previous-record fallback (`C2.carry_resolved`). This could be stable splitting a frame or re-judge timing; it cannot be told apart without stable source. |
| **H5** totals | **confirmed** | I6 4,637 pass, 9 fail, 36 na (19 osu!std plus 17 charts missing from osu!.db). All 9 failures are `short_total`, the same files as I5. No `.osg` play falls into `v1_split_ln` or the failed class: `C.all_mania.split_ln` = 0, `failed` = 0. X-cov finds all 11 `v1_split_ln` replays **without** an `.osg`. |
| **H6** V1 LN ticks | **confirmed** | C3 v1_ln: combo rises by more than the judgement count on 33.1% of records, +0.88 per record on average. 33 of 210 plays end with max combo above the judgement total. v1_rice_nm shows 2.8% (charts under 30% LN still have some LNs), and v2_ln / v2_rice_nm show 0.0%. |
| **H7** HP | **partial: not refuted, not confirmable** | X-fld: `hp_raw` ≤ 200 in 4,682 / 4,682 files. On passes (C `all_mania.C4.passed`, n = 4,646) the last value has median 200 and the minimum has median 161; 1 play touches 0 and none ends at 0. **No failed play has an `.osg`**: C4 `failed.n` = 0 in every group, and all 88 short-total `.osr` lack one (`failed_without_osg.failed`). So the "reaches 0 in failed plays" half cannot be tested. The spec's "14 files touch 0, 1 ends at 0" came from osu!std files (X-fld: std 13 touch and 1 ends at 0; mania 1 and 0). The name stays provisional. |
| **H8** V2 floats | **partial** | C5 all_mania: `f1` ≠ 0 in 0 of 6,774,060 V2 records. On single-judgement non-miss records, Δf0 = w(kind) × 150·log2(max(combo_after, 2)) with w = MAX 1, 300 1, 200 2/3, 100 1/3, 50 1/6. Records within 1e-6 of that ratio: MAX 95.8%, 300 97.0%, 200 98.0%, 100 99.0%, 50 99.7%. v2_rice_nm fits worse (MAX 85.1%, 300 89.9%). The misfits are unexplained. `f1` is unknown. |
| **Negative**: no column or object index | **confirmed** | Layout above. C2 below measures how far a (time, kind) assignment gets. |

### I5 / I6 / I7 exception class `final_short` (9 files)
Mods: V2 = 1<<29, HR = 16, HT = 256. "Missing" is `.osr` minus `.osg`.

| File | System | Mods | Missing | Score short by |
|---|---|---|---|---|
| `1965fef9…-134320151161493537` | V2 | V2 | 1 MAX | 321 |
| `19cbabce…-134335407814708904` | V2 | V2 | 1 × 300 | 153 |
| `2a9cee20…-134223362752485270` | V2 | V2+HR | 1 × 200 | 0 |
| `6735f698…-134340830243011208` | V2 | V2 | 1 MAX | 697 |
| `79f20941…-134332859765241089` | V2 | V2+HR | 2 MAX | 790 |
| `7eb84b43…-134333656447452510` | V1 | none | 1 MAX | 285 |
| `c0fad035…-134312068434377459` | V2 | V2 | 1 MAX | 450 |
| `e482eb04…-134337925197020302` | V2 | V2+HR | 1 × 300 | 716 |
| `fe1ebd1c…-134339020126720364` | V2 | V2+HR+HT | 1 × 300 | 367 |

Every missing judgement is a hit (MAX, 300 or 200), never a miss. That fits the last one or two updates not being flushed. It was not checked against re-judge which object is missing.

## Correlations per group (C)
C1 is the share of judgement units whose `t_ms` exactly equals a re-judge input instant of the same kind. C2 uses a (time, kind) join with ±1 ms tolerance:
- **uniq** = units assigned to exactly one object ÷ all units;
- **unm** = units with no candidate ÷ all units;
- head / tail / note = unique ÷ (unique + ambiguous) inside that category;
- **exact** = plays whose re-judge totals equal the `.osr` header.

In V1 the single LN judgement is emitted at release, so it counts as a tail.

| Group | n | V2 | exact | C1 | C1 time only | C2 uniq | C2 unm | head | tail | note | C3 >0 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| all_mania | 4,646 | 2,095 | 1,851 | 0.971 | 0.977 | 0.959 | 0.015 | 0.980 | 0.987 | 0.970 | 0.030 |
| v1_rice_nm | 2,139 | 0 | 1,589 | 0.986 | 0.986 | 0.968 | 0.003 | — | 0.985 | 0.970 | 0.028 |
| v1_ln | 210 | 0 | 4 | 0.955 | 0.960 | 0.942 | 0.040 | — | 0.990 | 0.977 | 0.331 |
| v2_rice_nm | 117 | 117 | 34 | 0.987 | 0.988 | 0.961 | 0.004 | 0.976 | 0.985 | 0.966 | 0.000 |
| v2_ln | 1,532 | 1,532 | 4 | 0.958 | 0.969 | 0.952 | 0.024 | 0.979 | 0.987 | 0.978 | 0.000 |
| dt_nc | 181 | 132 | 18 | 0.939 | 0.958 | 0.953 | 0.041 | 0.995 | 0.996 | 0.993 | 0.049 |
| ht | 77 | 22 | 31 | 0.963 | 0.970 | 0.928 | 0.025 | 0.966 | 0.975 | 0.945 | 0.058 |
| hr | 302 | 282 | 116 | 0.981 | 0.984 | 0.963 | 0.007 | 0.975 | 0.985 | 0.971 | 0.004 |
| mirror | 121 | 38 | 58 | 0.973 | 0.977 | 0.960 | 0.011 | 0.976 | 0.986 | 0.970 | 0.035 |
| ez | 2 | 0 | 0 | 0.869 | 0.890 | 0.878 | 0.115 | — | 0.997 | 0.990 | 0.262 |

- **Records with several judgements (all_mania C1 `by_n`).** Records carrying 1 judgement match exactly 97.5% of the time; 2 judgements 90.7%; 3 or more 88.2%. Units whose candidates span several categories (`mixed`) are 42,698, all ambiguous.
- **Skipped (C).** 19 osu!std plays; 17 plays whose chart is missing from osu!.db. Random-mod plays are skipped by design; the pilot has none.
- **Reading C1 and C2.** Both measure the `.osg` against `rejudge.py`, which is itself wrong on LN. Only 4 / 1,532 V2 LN plays and 4 / 210 V1 LN plays have re-judge totals equal to the header (`rejudge_exact`), against 1,589 / 2,139 on V1 rice. Every C1/C2 residual is therefore an upper bound on `.osg` error, not a measurement of it.

## Coverage
Every `Data/r` `.osr` (X-cov, matching S-rs `osr_without_osg` and C `failed_without_osg`):

| Class | With `.osg` | Without `.osg` |
|---|---|---|
| Before 2026-04-25 (predates the format) | 0 | 47 |
| Since then: complete mania play | 4,646 | 201 |
| Since then: failed play, short total (R4) | 0 | **88** |
| Since then: `v1_split_ln` (R3) | 0 | **11** (dated 2026-06-08 to 2026-07-08) |
| Since then: osu!std | 19 | 0 |
| Since then: chart not in osu!.db | 17 | 1 |
| **Total** | **4,682** | **348** (301 since 2026-04-25) |

Mania plays since the first `.osg` (X-cov; self scope = `TWulfZ` and `TWulfZasdasdasd d jSS||`, as in spec 004 AC15):

| Scope | With `.osg` | Without | Coverage | Coverage excluding failed plays |
|---|---|---|---|---|
| Self | 2,774 | 249 (59 failed, 11 split-LN, 178 complete, 1 unknown chart) | **91.8%** | 93.6% |
| Others | 1,889 | 52 (29 failed, 23 complete) | 97.3% | — |
| All | 4,663 | 301 | 93.9% | — |

- Self misses by month: 2026-05 45, 2026-06 112, 2026-07 23, 2026-08 29, 2026-09 40. They spread across every client build (S-rs `osr_without_osg.by_version`).
- What is known:
  - stable does not write an `.osg` for a failed play;
  - it did not write one for any of the 11 ScoreV1 split-LN plays;
  - it skips 201 of 4,847 complete mania passes (4.1%; self scope 178, about 6%), across several players and client builds.
- **Why complete passes are skipped is unknown.**

## Corrections to earlier notes
- **Research 03** (L150, L169) says the leading int is always 20260924 and the records have "incrementing indices". In fact the leading int is the **client build**: seven values, 20260312 … 20260924, each equal to the paired `.osr` version. The "indices" are **cumulative judgement counts and combo**.
- **Research 03** (L169, L333) counted 4,666 `.osg` files. The corpus has grown to 4,682.
- **Spec 006 Risks** needs three corrections:
  - "b25 is 0 in all but one sampled record": b25 is the final-record full-combo flag (36 files).
  - "the 0-judgement record is typically the last one": in mania, zero-judgement records occur only first (15) or last (2,213). The 10,026 mid-file ones are all osu!std.
  - "14 files touch HP 0": 13 of those are osu!std; one mania play touches 0.
- **Spec 006 H1** says "b25 (0)" and I3 says "b25 = 0". Both are wrong as written: see H1 above. As a result, `osg.nonzero_reserved` fires on every FC play (36 in the corpus), and spec AC8 "I3 on 100%" cannot pass until b25-on-final is reclassified in the codec and the survey.
- **Spec 006 T9 sanity check** expected C1 = 100% on the first 50 events of all three sample files. Only 2 of 3 met it; `0012f2ce…` gives 47 / 50, because 3 units in 2-judgement records are carried (H4).

## Remaining unknowns
1. Why stable skips the `.osg` for 4.1% of complete passes, and for every ScoreV1 split-LN play. Candidates, none tested: the results-screen path, quick retry, replay save from the watch screen, or the build-specific split-LN scoring path.
2. The cause of the 9 `final_short` files: last-update flush, or a quit or retry race.
3. Whether `hp_raw` is the life bar. This is untestable on this client because failed plays have no `.osg`. The `.osr` life-bar string was not compared.
4. `f1` (always 0) and the 2–15% of V2 records whose Δf0 misses the combo model.
5. `b4` (always 0) and why one FC play lacks b25.
6. Whether the 0.1% carry (H4) is stable splitting one frame across two records or re-judge timing.
7. Whether other users' clients or later builds write `.osg` the same way. Only one install was examined, and the format may change without its header changing layout. The stride check (I1) is the only structural gate.
8. V2 records whose combo gain is below the judgement count without a miss: 25,125 in v2_ln (C3 `records_extra_lt0`). Unexplained.
