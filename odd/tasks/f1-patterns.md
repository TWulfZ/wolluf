# F1 7K pattern engine

Branch `feat/f1-patterns` from `feat/label-gold-set` @ fe8df21 (stacked on PR #6) · opened 2026-09-29 · worktree `../wolluf-patterns`

## Objective
Every parsed 7K chart is split into pattern segments using the ADR 0017 ids, cached per version key and visible via `wolluf chart show --segments`, ready to be measured against the gold set.

## Problem and why
Deliverable 2 of F1 (architecture §3 patterns crate, §9.2, §12 F1 row). Rules only: no ML (ADR 0002). The research gives the prior art: Interlude prelude `Patterns.fs` (MIT), MinaCalc `is_bracket` and base types (MIT), and their known 7K weaknesses: middle column assigned to the left hand, no roll/trill recognisers (research 01 l.54, 02 l.7–44).

## Scope
- Authorized:
  - new `crates/patterns` (`wolluf-patterns`, domain: core + chart only);
  - an engine `patterns` stage (VERSION, golden, vkey with layout id + params hash);
  - store cache `segment` table (bump `CACHE_SCHEMA_VERSION`);
  - app: patterns pass chained in `IndexLibrary` after `chart_parse`, plus a query;
  - CLI `chart show --segments`;
  - root Cargo.toml member/path.
- Out of scope:
  - `label_override` and relabel application (with deliverable 5);
  - eval harness and metrics (deliverable 4);
  - difficulty (deliverable 3);
  - thumb-side heuristic (deliverable 3, hand load);
  - TOML param packs (Rust `Default` params hashed canonically for now).

## Constraints
- Definitions: ADR 0017 (minijack = 2, longjack = 3+, bracket = simultaneous trills in one hand, chordbracket = moving 2–3-note shape without jacks, chords > 4 alternating = jumptrill). The thumb follows the user layout's hand.
- Thresholds are snap- and chart-time-relative where possible, so shapes are rate-invariant. All thresholds live in `PatternParams` (D17), with a canonical params hash in the vkey.
- Deterministic (D3): no HashMap in outputs, fixed-order resolution (priority param, then strength, then id).
- Each segment has one primary pattern plus secondary tags, and never splits a chord.
- The gold set is a test set: rules are never tuned on it while it is being built. No suggestions reach `wolluf label`.
- TDD strict. Delivery: large (> 400 lines); ask at close whether to ship a single PR or stacked PRs.

## Acceptance criteria
- Every rule has positive, negative and near-miss `chart!` tests; proptests pass (spans in bounds, no chord split, determinism, mirror symmetry where hand-agnostic) → `cargo nextest run -p wolluf-patterns`.
- `patterns` stage in the stage lock with a golden over fixtures → `cargo xtask stage-lock --check`.
- Pilot corpus: every parsed 7K chart is segmented with no failures, the per-pattern coverage table is printed, and the second run is memoized → corpus test.
- `wolluf chart show <md5> --segments` shows segment bands next to the playfield.

## Tasks
- [x] T1: `wolluf-patterns` skeleton: `ChartView` + `RowFeat` primitives (press/release/held masks, jacks, per-hand masks from Layout, gaps, direction/roll, snap from red lines, density), `PatternRule` trait, `Candidate`, `PatternParams` (Default + canonical hash). Fetch the Interlude `Patterns.fs` / primitives constants from YAVSRG source (MIT) for reference. Route: delegated. Tier: medium. Commit: `feat(patterns): add ChartView primitives, rule trait and params`
- [x] T2: jack rules (minijack, longjack, chordjack, anchor). Route: delegated. Tier: medium. Commit: `feat(patterns): add jack rules`
- [x] T3: stream rules (single, jumpstream, handstream, chordstream light/dense, roll, trill, jumptrill, split_trill, bracket, chordbracket). Route: delegated. Tier: medium. Commit: `feat(patterns): add stream rules`
- [x] T4: tech + speed rules (irregular, hand_imbalance, thumb, burst) and LN rules (density, chord, hybrid, shield, inverse gap, release timing). Route: delegated. Tier: medium. Commit: `feat(patterns): add tech, speed and LN rules`
- [x] T5: segmenter: merge, min/max length, overlap resolution by priority, purity, secondary tags. Route: delegated. Tier: medium. Commit: `feat(patterns): add segmenter with priority resolution`
- [x] T6: engine `patterns` stage + golden + vkey; store `segment` table; app pass in IndexLibrary + query; CLI `--segments`; corpus test. Route: delegated. Tier: high (persisted encoding + vkey). Commit: `feat(engine): add patterns stage, segment cache and --segments view`
- [ ] T7: close: full gates, corpus, docs, remove this document. Route: inline. Tier: passive. Commit: —

## Progress
- 2026-09-29 T1: RED unresolved imports → GREEN 22/22; workspace 674 passed, 10 members. Interlude prelude (MIT, d41fc216) was read for primitives; direction, roll and jacks are ported with a NOTICE line. The ADR 0017 counts (minijack 2, longjack 3+, jump 2, hand 3, dense 4+, jumptrill > 4) are named constants that cite the ADR, not params. `detect` takes `&ChartView` only, since the view carries the Layout.

- 2026-09-29 T2: RED unresolved rule types → GREEN 57/57; workspace 709 passed. "Jack-fast" means ≤ 500 ms and ≤ 1 beat, which makes it rate-invariant.
- Accepted changes:
  - chordjack is a run of ≥ 3 chord rows with ≥ 1 repeated column and at least one chord of 3+ notes, dropping Interlude's `(b<a||j<b)` because a chord repeated unchanged is a chordjack in 7K;
  - anchor = the same column every 2nd press row for ≥ 4 hits, excluding trills.
  - Overlap to settle in T5: a chord repeated unchanged yields both chordjack and per-column longjacks.

- 2026-09-29 T3: RED unresolved rule types → GREEN 96/96; workspace 748 passed. A shaped proptest generator was added so every rule actually fires.
- Hand-dependent rules verified under both 313 layouts: split_trill and bracket (MinaCalc `is_bracket` per hand bucket, thumb follows the layout).
- Stream gap: ≤ 250 ms and ≤ ½ beat.
- Overlaps T5 must resolve by priority:
  - split_trill > trill/jumptrill;
  - bracket ⊃ trills;
  - roll vs single;
  - jumptrill vs chordstreams;
  - chordbracket inside chordstream_light;
  - nested same-family runs.

- 2026-09-29 T4: RED unresolved rule types → GREEN 129/129; workspace 781 passed; all 25 rules fire in shaped proptests.
- Window rules (irregular, hand_imbalance, thumb, ln.density) produce wide spans. Plan for T5: treat them as secondary tags, not primary patterns.
- LN release stagger floor is 30 ms (lazer `release_threshold`).
- More overlaps for T5:
  - shield ⊂ jacks;
  - hybrid vs rice;
  - ln.chord/density/inverse on full LN blocks;
  - burst vs stream.

- 2026-09-29 T5: RED missing segment API → GREEN 153/153; workspace 805 passed. Each row gets exactly one winner:
  - a priority table (shield, inverse, release, hybrid, LN chord, chordjack, burst, split_trill, jumptrill, bracket, trill, chordbracket, roll, anchor, the chordstreams, hand-/jump-/single streams, longjack, minijack), with LN rules demoted when the LN share is < 25%;
  - withdrawn short runs re-resolve.

  Other behaviour:
  - Segments are 2–8 s (a short class is exempt), split at row boundaries, and merged across ≤ ½-beat gaps.
  - Tag-only rules and losing candidates overlapping ≥ 20% become secondary tags.
  - Snapshot `mixed_7k` accepted.
- Accepted changes:
  - jumptrill ranks above bracket, because a whole-row chord alternation reads as a jumptrill;
  - minijack/longjack rank last, so lone jacks do not cut streams;
  - burst is below chordjack;
  - `ln.inverse.gap` fix: a gap counts only if local coverage ≥ 60%;
  - all params are uncalibrated placeholders until the eval step.

- 2026-09-29 T6: RED per layer → GREEN; workspace 833 passed.
- The independent verifier asked for changes; the scoped correction was applied:
  - the patterns vkey folds in the chart_parse vkey and the sorted (rule id, version) pairs;
  - the golden covers all 25 ids (enforced by a test);
  - store canonicalizes secondary tags and adds a json_valid CHECK;
  - a missing parse row is an item failure, and a keymode mismatch is PARSE_FAILED;
  - the `--segments` legend names the segmenter layout.
- Frozen values: vkey `bc9713f9…`, golden `3d14fbc8…`.
- Accepted change: the engine re-exports `Chart` and `Segment` so app can name them, which adds no crate edge (the verifier agreed).

## Next step
T7 (remaining): with osu! closed run `WOLLUF_CORPUS="/mnt/e/Games/osu!" cargo nextest run -p wolluf-app --run-ignored only -E 'test(corpus_patterns)'` (and the full corpus), record the numbers here, then `git rm` this document and mark PR #7 ready. Docs and gates are already done (833 passed).
