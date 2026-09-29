-- cache.db at CACHE_SCHEMA_VERSION (architecture §5.4, spec 003 Data). Disposable: a schema change
-- bumps the constant and the file is rebuilt; there are no migrations, ever.
-- Times are RFC 3339 UTC strings with milliseconds, like user.db.

-- Memo of every derived item: staleness, per-item failures, "recompute only what changed".
CREATE TABLE derivation (
    stage       TEXT NOT NULL,
    input_key   TEXT NOT NULL,
    vkey        BLOB NOT NULL CHECK (length(vkey) = 32),
    status      TEXT NOT NULL CHECK (status IN ('ok', 'failed', 'skipped')),
    error_code  TEXT NULL,
    error_msg   TEXT NULL,
    duration_ms INTEGER NULL,
    PRIMARY KEY (stage, input_key, vkey)
) STRICT;

-- Mania entries of the latest osu!.db snapshot; `snapshot_id` points into user.db, so there is
-- no foreign key across files.
CREATE TABLE catalog_chart (
    md5         TEXT PRIMARY KEY,
    keymode     INTEGER NOT NULL,
    title       TEXT NOT NULL,
    artist      TEXT NOT NULL,
    version     TEXT NOT NULL,
    creator     TEXT NOT NULL,
    set_id      INTEGER NULL,
    beatmap_id  INTEGER NULL,
    path        TEXT NOT NULL,
    od          REAL NOT NULL,
    hp          REAL NOT NULL,
    length_ms   INTEGER NOT NULL,
    snapshot_id INTEGER NOT NULL
) STRICT;

CREATE TABLE job_run (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    params_json  TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('queued', 'running', 'ok', 'failed', 'cancelled')),
    started      TEXT NULL,
    ended        TEXT NULL,
    summary_json TEXT NULL
) STRICT;

CREATE TABLE item_failure (
    job_id   TEXT NOT NULL REFERENCES job_run (id),
    item_ref TEXT NOT NULL,
    code     TEXT NOT NULL,
    message  TEXT NOT NULL,
    PRIMARY KEY (job_id, item_ref)
) STRICT;

-- Per-alias listing stats for the identity wizard (spec 004 Data, architecture §5.6).
CREATE TABLE alias_stats (
    alias_id          INTEGER NOT NULL,
    vkey              BLOB NOT NULL CHECK (length(vkey) = 32),
    n_plays           INTEGER NOT NULL,
    n_by_keymode_json TEXT NOT NULL,
    first_ts          TEXT NULL,
    last_ts           TEXT NULL,
    n_with_replay     INTEGER NOT NULL,
    n_online_ids      INTEGER NOT NULL,
    top_charts_json   TEXT NOT NULL,
    PRIMARY KEY (alias_id, vkey)
) STRICT;

-- Normalized chart per chart_parse key. rows_blob is opaque here: the engine owns its encoding
-- and format-version header.
CREATE TABLE chart_parsed (
    md5       TEXT NOT NULL,
    vkey      BLOB NOT NULL CHECK (length(vkey) = 32),
    rows_blob BLOB NOT NULL,
    n_notes   INTEGER NOT NULL,
    n_ln      INTEGER NOT NULL,
    ln_ratio  REAL NOT NULL,
    length_ms INTEGER NOT NULL,
    PRIMARY KEY (md5, vkey)
) STRICT;

-- Source labels read from local difficulty names (research 03). A chart can sit on several
-- scales, and on one scale more than once only under different level texts.
CREATE TABLE chart_label (
    md5        TEXT NOT NULL,
    vkey       BLOB NOT NULL CHECK (length(vkey) = 32),
    source     TEXT NOT NULL,
    scale      TEXT NOT NULL,
    level_ord  REAL NULL,
    level_text TEXT NOT NULL,
    skill_tag  TEXT NULL,
    is_variant INTEGER NOT NULL CHECK (is_variant IN (0, 1)),
    PRIMARY KEY (md5, vkey, scale, level_text)
) STRICT;

-- The primary key already serves per-md5 lookups; every read filters by vkey first (D15).
CREATE INDEX chart_label_scale_level ON chart_label (vkey, scale, level_ord);

-- Pattern segments per `patterns` key (ADR 0017 ids): at most one primary pattern per instant.
-- `idx` is the segment's rank in time order within its chart. `cols` is a column bitmask.
CREATE TABLE segment (
    md5            TEXT NOT NULL,
    vkey           BLOB NOT NULL CHECK (length(vkey) = 32),
    idx            INTEGER NOT NULL CHECK (idx >= 0),
    t0_us          INTEGER NOT NULL,
    t1_us          INTEGER NOT NULL CHECK (t1_us >= t0_us),
    cols           INTEGER NOT NULL CHECK (cols BETWEEN 0 AND 65535),
    axis_id        TEXT NOT NULL,
    pattern_id     TEXT NOT NULL,
    secondary_json TEXT NOT NULL CHECK (json_valid(secondary_json)),
    purity         INTEGER NOT NULL CHECK (purity BETWEEN 0 AND 1000),
    strength       INTEGER NOT NULL CHECK (strength BETWEEN 0 AND 1000),
    PRIMARY KEY (md5, vkey, idx)
) STRICT;

CREATE INDEX segment_pattern ON segment (vkey, pattern_id);
