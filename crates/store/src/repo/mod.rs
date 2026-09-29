//! Repositories over user.db and cache.db. They take and return store and core types; SQL
//! never leaves this crate (D6).

pub mod cache;
pub mod labels;
pub mod ledger;
pub mod players;
mod sql;

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use crate::user::testkit::{BLOB_OSG, BLOB_OSR, PLAY_A, PLAY_B, count, migrated, seed};

    fn aborts(conn: &Connection, sql: &str, params: impl rusqlite::Params) {
        let err = conn.execute(sql, params).err();
        assert!(err.is_some(), "expected abort: {sql}");
    }

    #[test]
    fn play_ledger_immutable() {
        let conn = migrated();
        seed(&conn);
        aborts(&conn, "DELETE FROM play WHERE id = ?1", [PLAY_A]);
        aborts(
            &conn,
            "UPDATE play SET score = score + 1 WHERE id = ?1",
            [PLAY_A],
        );
        aborts(&conn, "UPDATE play SET passed = 1 WHERE id = ?1", [PLAY_A]);
        conn.execute(
            "UPDATE play SET replay_sha = ?1 WHERE id = ?2",
            (BLOB_OSR, PLAY_A),
        )
        .unwrap();
        aborts(
            &conn,
            "UPDATE play SET replay_sha = ?1 WHERE id = ?2",
            (BLOB_OSG, PLAY_A),
        );
        aborts(
            &conn,
            "UPDATE play SET replay_sha = NULL WHERE id = ?1",
            [PLAY_A],
        );
        conn.execute(
            "UPDATE play SET osg_sha = ?1 WHERE id = ?2",
            (BLOB_OSG, PLAY_A),
        )
        .unwrap();
        assert_eq!(count(&conn, "play"), 2);
    }

    #[test]
    fn play_origin_constraints() {
        let conn = migrated();
        seed(&conn);
        let insert = "INSERT INTO play (id, alias_id, chart_md5, origin, filetime, played_at_utc,
                mods, score_system, counts_json, max_combo, score, client_version, replay_sha,
                snapshot_id, ingested_at)
            VALUES (?1, 1, 'e956977ccc1d74a50ae48b43a868cc20', ?2, '1', 't', 0, 'v1', '{}', 0, 0,
                20260924, ?3, ?4, 't')";
        let no_blob: Option<[u8; 32]> = None;
        let no_snapshot: Option<i64> = None;
        aborts(
            &conn,
            insert,
            ([0xc3_u8; 32], "replay_only", no_blob, no_snapshot),
        );
        aborts(
            &conn,
            insert,
            ([0xc4_u8; 32], "scores_db", Some(BLOB_OSR), no_snapshot),
        );

        // scores_db -> replay_only is never allowed.
        aborts(
            &conn,
            "UPDATE play SET origin = 'replay_only', snapshot_id = NULL WHERE id = ?1",
            [PLAY_A],
        );
        // The upgrade must set snapshot_id in the same statement.
        aborts(
            &conn,
            "UPDATE play SET origin = 'scores_db' WHERE id = ?1",
            [PLAY_B],
        );
        conn.execute(
            "UPDATE play SET origin = 'scores_db', snapshot_id = 1 WHERE id = ?1",
            [PLAY_B],
        )
        .unwrap();
        aborts(
            &conn,
            "UPDATE play SET snapshot_id = NULL WHERE id = ?1",
            [PLAY_B],
        );
    }

    #[test]
    fn feedback_event_append_only() {
        let conn = migrated();
        seed(&conn);
        aborts(&conn, "DELETE FROM feedback_event", []);
        aborts(
            &conn,
            "UPDATE feedback_event SET payload_json = '{\"x\":1}'",
            [],
        );
        aborts(&conn, "UPDATE feedback_event SET kind = 'rating'", []);
        conn.execute("UPDATE feedback_event SET telemetry_state = 'sent'", [])
            .unwrap();
        aborts(
            &conn,
            "UPDATE feedback_event SET telemetry_state = 'lost'",
            [],
        );
        assert_eq!(count(&conn, "feedback_event"), 1);
    }
}
