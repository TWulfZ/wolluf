# 0014 F0 ledger refinements

- Status: Accepted (2026-09-29)
- Date: 2026-09-28

## Context
Spec 003 (store, play ledger and SyncPlays) turns the read-only osu! stable install into wolluf's own ledger. Writing it against the real pilot data exposed six places where the baseline architecture (§3, §5.3, §7) is either self-contradictory or wrong about the data:

1. **Snapshot mechanism vs D9.** §3 describes `source-osu` as doing a "snapshot-copy of DBs", and §7 says osu!.db, scores.db and collection.db are "snapshot-copied to temp before parsing". D9 says `source-osu` has *no fs write calls*, and check-layers L6 bans `fs::write`, `File::create`, `fs::copy` and friends in that crate. A temp copy is a write. The files are small enough to hold in memory: osu!.db 31.2 MB, scores.db 652 KB, collection.db 3.5 KB on the pilot install (measured 2026-09-28).
2. **`play.passed` cannot be filled.** scores.db has no pass flag and its lifebar string is empty. On client 20260924 failed plays *are* saved to scores.db and `Data/r` (research 03 l.130 and l.160: 79 confirmed fails, none before 2026-04-25). Pass/fail needs chart object counts or the replay life graph, which is F2 work. §5.3 lists `passed` as an ordinary, implicitly non-null column.
3. **Alias names are bytes.** §5.3 says "raw bytes are the key", but the column type is not stated. Names can be empty (`""` is the pilot's second-largest alias) and scores.db strings are not guaranteed UTF-8.
4. **What `play.filetime` holds.** §5.3 names the key `blake3(game, chart_md5, raw_name, filetime)`. scores.db and the `.osr` header store .NET ticks (100 ns since 0001-01-01), while the `Data/r` file name suffix is FILETIME (100 ns since 1601-01-01) = ticks − 504 911 232 000 000 000, matching 4362/4362 replays (research 03 l.168). The id and column need one fixed representation.
5. **Timezone of scores.db ticks.** `played_at_utc` needs the ticks' zone. The newest pilot score decodes to 2026-09-28 23:13:56 if read as UTC, while the scores.db mtime is 2026-09-28 18:18:56 −05:00 = 23:18:56 UTC: the file was written 5 minutes after the play, which only fits if ticks are UTC (read as local time the play would lie 5 hours in the future).
6. **Orphan replays.** 42 `.osr` files in the pilot's `Data/r` have no scores.db row; all are mania and their header timestamp equals the name (measured 2026-09-28). They are most likely plays whose score rows were deleted. §5.3 has no way to represent a play that did not come from scores.db, and its `snapshot_id` column assumes every play came from a DB snapshot. The user decided on 2026-09-28 that a replay without a score row counts as a play.

## Decision
1. **In-memory source snapshots.** `source-osu::snapshot::read_stable` stats the file, reads it fully into memory with `fs::read`, stats it again, and retries with backoff (250 ms, 500 ms, 1 s) while size or mtime changes; after 3 changed reads it fails with `OSU_RUNNING`. The bytes, sha256, size and mtime form the snapshot, and parsing runs over the in-memory copy. No temp file is ever written, so D9 stays literal and L6 needs no exception. Architecture §3 and §7 are reworded to match.
2. **`play.passed` is nullable** (`INTEGER NULL CHECK(passed IN (0,1))`) and stays NULL for every play ingested in F0. F2 derives pass/fail as a cache.db stage; the ledger column is kept for a later confirmed fact, never for a guess.
3. **`alias.raw_name` is a `BLOB`**, compared bytewise, with `UNIQUE(game, raw_name)`. `''` is a valid alias and non-UTF-8 bytes are preserved. Normalisation (spec 004) is computed for matching only and is never stored as a key.
4. **`play.filetime` is `FileTime` decimal `TEXT`**, the same text as the `Data/r` suffix. Ticks are converted once at ingest through `core::DotNetTicks::to_filetime`; ticks before 1601-01-01 are an item failure (`INVALID_INPUT`). `play.id = PlayId::derive(Game, ChartMd5, raw_name, FileTime)`, whose byte encoding is pinned by **ADR 0006** and not restated here. TEXT keeps the full 64-bit value exact for SQLite tooling and for IPC, where ids above 2^53 travel as strings (§8).
5. **scores.db and `.osr` header ticks are UTC.** `played_at_utc` is derived from them with no timezone conversion. The mtime evidence above is the pin; if a future client disagrees, this ADR is superseded.
6. **`play.origin` (`scores_db | replay_only`).** Every play records where it came from:
   - `scores_db`: built from a scores.db record; `snapshot_id` is NOT NULL.
   - `replay_only`: an orphan `.osr` in `Data/r` whose header agrees with its name (beatmap md5 and ticks → `FileTime`), has mode 3, and has no matching scores.db record. The play is built from the header, which uses the scores.db record layout: alias from the header player name, counts, mods, score, max combo, `client_version`, online score id, and `filetime`/`played_at_utc` from the header ticks. It uses **the same natural key**. `replay_sha` is NOT NULL (the file is vaulted first) and `snapshot_id` is NULL.
   - The only permitted change is the one-way upgrade `replay_only → scores_db` with `snapshot_id` NULL → value, when a later scores.db snapshot contains an identical record (e.g. a sync that ran between osu!'s two writes). DB triggers enforce this together with the NULL → value rule for `replay_sha`, `osg_sha` and `chart_sha`.
   - An orphan whose header disagrees with its name, is from an unsupported client, or is not mania is not turned into a play; the bytes are still vaulted unless the header is non-mania.

## Alternatives considered
- **Temp-file snapshots (the §7 wording):** rejected. They need fs writes in `source-osu`, contradicting D9 and L6, and buy nothing at these file sizes. Moving the copy into `app` would put osu!-format knowledge outside the adapter and still write a temp file.
- **Reading osu!'s files in place and parsing while streaming:** rejected. A concurrent osu! write could yield a torn parse with no way to detect it; the stat–read–stat check needs the whole file in hand.
- **Inferring `passed` in F0 from the header totals vs the chart object count:** rejected. It needs the parsed chart (F1) and is wrong for the V1 HR LN split head/tail window found in research 03, so a guess would enter an immutable ledger.
- **`raw_name` as `TEXT`:** rejected. SQLite TEXT invites implicit UTF-8 assumptions and collation surprises; the key is bytes by definition (§5.3).
- **Storing ticks instead of FILETIME:** rejected. §5.3 names FILETIME, the `Data/r` join uses FILETIME, and the two are bijective from 1601 on, so the choice only fixes one spelling. `INTEGER` storage was rejected for the IPC precision reason.
- **Treating ticks as local time:** rejected by the mtime evidence.
- **Ignoring orphan replays, or vaulting them as blobs only:** rejected by the user decision. They are real plays with a replay, and dropping them loses history once scores.db no longer has them.
- **A separate `orphan_replay` table:** rejected. Every consumer (identity stats, evidence, sessions) would need a union, while `origin` keeps them in one ledger and still lets a later phase weigh them differently.

## Consequences
- `source-osu` never writes to disk, and check-layers L6 keeps proving it. Peak memory during a sync includes one osu!.db copy (~31 MB on the pilot).
- Every consumer of `play.passed` must handle NULL until F2 lands its derivation.
- `play.filetime` and `play.id` are frozen: changing either would duplicate the ledger. ADR 0006 owns the id encoding.
- The play ledger has two origins but one key space, so re-ingest stays idempotent: `sync_twice_adds_zero_rows` and `replay_only_upgraded_when_score_row_appears` (spec 003) guard it.
- If osu! ever writes a scores.db row whose fields differ from an existing `replay_only` play with the same key, the stored row wins and an `item_failure(CONFLICT)` is recorded, as for any other conflicting duplicate.
- Architecture §3, §5.3 and §7 are updated in the same change as this ADR.
