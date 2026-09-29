# 0012 .osg handling

- Status: Accepted (2026-09-29)
- Date: 2026-09-28

## Context
Architecture §13 O1 asks what the `.osg` files in `Data/r` contain. The answer decides whether F2 judges LN from stable's own record or keeps re-judging it. Re-judging LN is the weakest part of the F2 plan: ScoreV1 mid-body LN mechanics are unknown, and ScoreV2 LN reaches 63% parity (research 03 L145–L148). Spec 006 ran a 3-day spike. It produced a Rust decoder and survey, a Python oracle and a correlation against `rejudge.py`. The findings, with every number sourced, are in `docs/research/04-osg-format.md`. The ones that decide this ADR:

- **What the file is.** It is a per-play judgement timeline: an `i32` client build, an `i32` record count, then one fixed 29-byte record per score-changing update (45 bytes under ScoreV2). Each record carries the map-time instant, cumulative counts in `.osr` order, score, max combo, combo, a 0–200 life value and a V2 flag. It has **no column and no object index**. Structure holds on 4,682 / 4,682 files. The byte at offset 25 is not reserved: it is a final-record full-combo flag (36 of 37 FC plays).
- **Fidelity.**
  - The last record equals the `.osr` header on 4,673 / 4,682 files (99.81%). The 9 others (`final_short`) stop 1–2 hit judgements short.
  - Record times equal re-judge's input instant for 97.1% of judgement units, or 98.3% counting expiry misses. The residual is bounded by the oracle: re-judge reproduces stable's totals on only 4 of 1,532 V2 LN plays.
- **Assignability (C2).** A (time, kind) join with ±1 ms tolerance assigns units to exactly one object as follows:

  | Group | Heads (of matched) | Tails (of matched) | Of all units |
  |---|---|---|---|
  | V2 LN | 97.9% | 98.7% | 95.2% |
  | V1 LN (one judgement per LN, at release) | — | 99.0% | 94.2% |
  | All mania | 98.0% | 98.7% | 95.9% |

- **Coverage.**
  - Stable writes no `.osg` for a failed play: all 88 short-total `.osr` lack one, and 0 of 4,646 analysed `.osg` plays failed.
  - It wrote none for the 11 ScoreV1 split-LN plays.
  - It skips 201 of 4,847 complete passes (4.1%) for an unknown reason.
  - Self-scope coverage since the first `.osg` (2026-04-25) is 2,774 / 3,023 = **91.8%**, or 93.6% excluding failed plays.
- **Size.** 4,682 files take 511 MB. On a seeded 200-file sample, zlib-9 compresses them to 37% and xz-6 to 25%. zstd was not measured (no binary on the machine).
- **Version header.** The leading `i32` is the client build: seven values in six months, each equal to the paired `.osr` version. As with ADR 0015, it changes on every osu! update, whether or not the layout changes.

Spec 006 wrote the decision criteria before the measurements. The thresholds are spike verdicts only and never enter algorithm code (D17):

| Option | Requires | Measured | Met |
|---|---|---|---|
| **O1a** `.osg` is the judgement source; LN re-judging is dropped | H1–H4 confirmed | H1 confirmed with the b25 correction; H2, H3 and H4 confirmed, each with a small exception class | yes, with caveats |
| | C2 unique ≥ 99% for heads and tails on V1 LN and V2 LN | best case (of matched): V2 heads 97.9%, V2 tails 98.7%, V1 tails 99.0%; of all units: 95.2% / 94.2% | **no** |
| | coverage ≥ 95% of self-scope plays since 2026-04-25 | 91.8% (93.6% excluding failed plays) | **no** |
| **O1c** hybrid | H1–H3 confirmed | yes; the fields O1c reads (time, counts, score, combo) are all confirmed | yes |
| | C2 ≥ 90% | 94.2% (V1 LN) and 95.2% (V2 LN) of all units; 97.9–99.0% of matched | **yes** |
| **O1b** not usable | H1 or H2 refuted, or C2 < 90% | neither | no |

O1a fails on both measured criteria, under either reading of C2 (share of all units, or share of matched units). The coverage failure is structural: failed plays never get an `.osg`, so O1a could never cover them.

## Decision
**Recommend O1c (hybrid).** In F2, the `.osg` is a per-event oracle beside the re-judge engine:

1. **Decode stage.** F2's `replay_decode` stage owns an `.osg` decoder `VERSION`, and its output lands in `replay_input` (architecture §5.4, §5.5; ADR 0006). It decodes vault bytes, never the live `Data/r` file.
2. **Assignment.** For every play that has an `.osg`, events are joined to the chart objects produced by re-judge candidate generation. The join uses time and kind, with the same-frame carry to the previous record, as in `correlate.py`.
   - An LN head, LN tail or note whose `.osg` event assigns **uniquely** takes stable's judgement from the `.osg`, tagged with high confidence (the tag vocabulary belongs to ADR 0011).
   - Ambiguous or unmatched units fall back to re-judge with its normal confidence.
   - The join tolerance and carry rule are params, not inline constants (D17).
3. **Parity check.** Where an `.osg` exists, the re-judge output is compared per event, not only on totals. This parity metric replaces "totals equal" for LN in the F2 harness.
4. **Plays without an `.osg`** (failed plays, the unexplained 4% of passes, everything before 2026-04-25, other clients) use re-judge alone.
5. **`final_short` files** (last record short of the `.osr` header) are still used per event. Their missing final judgement(s) come from re-judge, and the play is flagged in diagnostics.
6. **Version gate (R7).** An unknown client build in the `.osg` header is a **warning** (`osg.unknown_client_version`), never `UNSUPPORTED_FORMAT`. The structural gate is the stride check: (len − 8) = count × stride, stride ∈ {29, 45}, with stride agreeing with the `.osr` ScoreV2 bit. This applies ADR 0015's "header = client build" reasoning to `.osg`. The difference: `.osg` has no `min` / `newest_verified` pair, because no layout threshold is known, and a stride failure is `PARSE_FAILED` (spec 006 R7).
7. **b25.** b25 is a final-record full-combo flag. The codec must stop reporting b25 = 1 on the final record as `osg.nonzero_reserved`. b25 anywhere else, and b4 ≠ 0 anywhere, stay warnings. b25 is not an FC oracle in either direction: one FC play lacks it, so FC status comes from the `.osr` header.

**Vault (answers 003 O9 for `.osg`).** `.osg` bytes are **irreplaceable**. They record stable's own judgements, and re-judging does not reproduce them: 0.26% exact totals on V2 LN. The vault keeps archiving every `.osg`, raw and content-addressed, as it does today (architecture §5.2). Compression at rest (zstd or similar, 25–37% of raw by the proxies measured) is allowed later as a storage-layer change. It must keep sha256 over the raw bytes, so blob identity does not change. It is not needed in F0: 511 MB on the pilot.

## Alternatives considered
- **O1a: `.osg` as the only judgement source; drop LN re-judging.** Rejected on the measured criteria.
  - Assignment is only 94–95% unique over all units, so a pure-`.osg` path would still need re-judge candidates to place events on columns.
  - Coverage is 91.8% in self scope, and failed plays never have an `.osg`.
  - F2 would still need a full re-judge engine for uncovered plays, so O1a saves no engine while losing the per-event parity check.
- **O1b: `.osg` not usable; offset-based LN metrics with confidence tags only.** Rejected. H0–H3 hold on the corpus and C2 clears 90%. Ignoring the `.osg` would throw away stable's own judgement on about 95% of LN events of covered plays, the part of F2 the plan is weakest on.
- **O1c without a vault copy (read `.osg` from `Data/r` on demand).** Rejected. osu! can delete or overwrite replays, and a decoder fix must be replayable from stored bytes (ADR 0003; architecture §5.2 and Appendix A, "Decoded replay events vs original bytes").
- **Treating an unknown `.osg` client build as `UNSUPPORTED_FORMAT`, as `.osr` and the DBs do above `newest_verified`.** Rejected: the header changes on every client update (seven builds in six months). The stride check fails loudly on a real layout change, and no `.osg` layout threshold is known to key a `newest_verified` on.

## Consequences
- **F2 scope.** It keeps the full re-judge engine, adds an `.osg` event-assignment step and a per-event parity report, and gains a high-confidence LN judgement for covered plays. The architecture §12 F2 row already lists ".osr/.osg decode stage".
- **Architecture edits on acceptance** (ARCH lane, same PR as the status change):
  - §13 O1 gets the O1c outcome and a link here;
  - O9 records that `.osg` stays archived.
- **The spec 006 close must reconcile the b25 finding.**
  - AC8's "I3 on 100%" cannot hold as written. It should become "I3 on 100% after reclassifying b25-on-final".
  - The codec and survey need the reclassification of decision item 7.
  - Until then, `osg.nonzero_reserved` fires on every FC play (36 in the corpus), and `wolluf osg survey --strict` reports those 36 as unexplained, together with the 9 `final_short` files under I5 and I7.
- **Must hold true.** `.osg` bytes are never dropped from the vault while this ADR stands. Any decode is a versioned derivation. F2 never uses `.osg` data for plays outside the selected identity scope in self profiles (§5.6).
- **Open.** The skip trigger for complete passes, the `final_short` cause, `hp_raw` naming (untestable because fails have no `.osg`) and `f1` stay unknown (04 "Remaining unknowns"). Only one client install was examined. Behaviour on other users' clients must be re-surveyed with `wolluf osg survey` before F2 relies on the coverage numbers.
