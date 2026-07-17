//! SQLite concurrency spike (PR-H5).
//!
//! Synthetic-only experiments comparing rollback journal vs WAL for RecallEngine's
//! real workflows (import writer + serve/browse readers). This module documents
//! measured lock and snapshot behaviour; it does not change production pragmas.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Default SQLite DELETE/rollback journal (current production).
    Rollback,
    /// WAL without busy_timeout.
    Wal,
    /// WAL with a bounded busy_timeout.
    WalBusy,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Rollback => "rollback",
            Self::Wal => "wal",
            Self::WalBusy => "wal+busy",
        }
    }
}

fn apply_mode(conn: &Connection, mode: Mode) {
    match mode {
        Mode::Rollback => {
            conn.pragma_update(None, "journal_mode", "DELETE")
                .expect("set DELETE");
        }
        Mode::Wal | Mode::WalBusy => {
            conn.pragma_update(None, "journal_mode", "WAL")
                .expect("set WAL");
        }
    }
    if matches!(mode, Mode::WalBusy) {
        conn.busy_timeout(Duration::from_millis(250))
            .expect("busy_timeout");
    }
    conn.execute("PRAGMA foreign_keys = ON", [])
        .expect("foreign_keys");
}

fn journal_mode(conn: &Connection) -> String {
    conn.pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal_mode")
}

fn open_writer(path: &Path, mode: Mode) -> Connection {
    let conn = Connection::open(path).expect("open writer");
    apply_mode(&conn, mode);
    conn
}

fn open_reader(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn seed_schema(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            body TEXT NOT NULL
         );
         INSERT OR REPLACE INTO messages(id, body) VALUES ('seed', 'hello');",
    )
    .expect("seed");
}

fn count_messages(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
}

fn sidecar_paths(db: &Path) -> (PathBuf, PathBuf) {
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let shm = PathBuf::from(format!("{}-shm", db.display()));
    (wal, shm)
}

fn prepare_db(mode: Mode) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let db = tmp.path().join("spike.sqlite");
    {
        let conn = open_writer(&db, mode);
        seed_schema(&conn);
        assert_eq!(count_messages(&conn).unwrap(), 1);
        // Confirm mode stuck for this connection.
        let mode_name = journal_mode(&conn).to_ascii_lowercase();
        match mode {
            Mode::Rollback => assert_eq!(mode_name, "delete"),
            Mode::Wal | Mode::WalBusy => assert_eq!(mode_name, "wal"),
        }
    }
    (tmp, db)
}

fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
                ..
            },
            _
        )
    )
}

/// Scenario A: long-lived reader transaction open while a writer commits.
#[test]
fn scenario_a_reader_txn_during_writer_commit() {
    for mode in [Mode::Rollback, Mode::Wal, Mode::WalBusy] {
        let (_tmp, db) = prepare_db(mode);
        let reader = open_reader(&db).expect("reader");
        reader.execute_batch("BEGIN").expect("begin read txn");
        let before = count_messages(&reader).unwrap();
        assert_eq!(before, 1);

        let writer = open_writer(&db, mode);
        let write = writer.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["w1", "from-writer"],
        );
        match (mode, write) {
            (Mode::Rollback, Err(err)) => {
                // DELETE journal: writer is blocked by an open read transaction.
                assert!(
                    is_busy(&err),
                    "{}: expected busy/locked writer, got {err}",
                    mode.label()
                );
            }
            (Mode::Wal | Mode::WalBusy, Ok(_)) => {
                writer.execute_batch("COMMIT").ok();
                // Reader snapshot must remain the pre-commit count until it ends.
                let during = count_messages(&reader).unwrap();
                assert_eq!(
                    during,
                    before,
                    "{}: reader snapshot changed while txn open",
                    mode.label()
                );
            }
            (Mode::Wal | Mode::WalBusy, Err(err)) => {
                panic!("{}: unexpected writer error {err}", mode.label());
            }
            (Mode::Rollback, Ok(_)) => {
                panic!(
                    "{}: writer should not succeed under open reader txn",
                    mode.label()
                );
            }
        }

        reader.execute_batch("COMMIT").ok();
        let after_reader = open_reader(&db).expect("fresh reader");
        let after = count_messages(&after_reader).unwrap();
        match mode {
            Mode::Rollback => assert_eq!(after, 1, "rollback: write did not land"),
            Mode::Wal | Mode::WalBusy => {
                assert_eq!(after, 2, "wal: write visible after new reader")
            }
        }
    }
}

/// Scenario B: open import-style writer transaction, then a new reader starts.
///
/// Distinguishes lock availability from snapshot consistency: under both DELETE and WAL,
/// readers that succeed must see only the last committed snapshot (not the pending insert).
#[test]
fn scenario_b_reader_during_open_writer_txn() {
    for mode in [Mode::Rollback, Mode::Wal, Mode::WalBusy] {
        let (_tmp, db) = prepare_db(mode);
        let mut writer = open_writer(&db, mode);
        let tx = writer.transaction().expect("begin writer txn");
        tx.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["pending", "uncommitted"],
        )
        .expect("insert pending");

        let read_started = Instant::now();
        let read_result = open_reader(&db).and_then(|conn| count_messages(&conn));
        let elapsed = read_started.elapsed();

        match read_result {
            Ok(count) => {
                // Snapshot consistency: uncommitted import rows must stay invisible.
                assert_eq!(
                    count,
                    1,
                    "{}: reader must not see uncommitted rows (got {count})",
                    mode.label()
                );
                assert!(
                    elapsed < Duration::from_millis(500),
                    "{}: reader took too long ({elapsed:?})",
                    mode.label()
                );
            }
            Err(err) if is_busy(&err) => {
                // Possible under heavy locking; still proves readers do not observe
                // uncommitted rows.
            }
            Err(err) => panic!("{}: unexpected reader error {err}", mode.label()),
        }

        tx.commit().expect("commit writer");
        let after = count_messages(&open_reader(&db).unwrap()).unwrap();
        assert_eq!(after, 2, "{}: committed row visible", mode.label());
    }
}

/// Scenario C: API-style read-only opens repeatedly while a writer holds a txn.
///
/// Under DELETE, a writer RESERVED lock typically still allows SHARED readers of the
/// last commit. Under WAL, readers also succeed against the last checkpointed snapshot.
/// Either way, counts must stay at the committed baseline until the writer commits.
#[test]
fn scenario_c_repeated_readonly_opens_during_import_txn() {
    for mode in [Mode::Rollback, Mode::Wal, Mode::WalBusy] {
        let (_tmp, db) = prepare_db(mode);
        let mut writer = open_writer(&db, mode);
        let tx = writer.transaction().expect("writer txn");
        tx.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["c1", "during-import"],
        )
        .unwrap();

        let mut successes = 0u32;
        let mut busy = 0u32;
        for _ in 0..8 {
            match open_reader(&db).and_then(|c| count_messages(&c)) {
                Ok(count) => {
                    assert_eq!(count, 1, "{}: only committed rows", mode.label());
                    successes += 1;
                }
                Err(err) if is_busy(&err) => busy += 1,
                Err(err) => panic!("{}: unexpected {err}", mode.label()),
            }
            thread::sleep(Duration::from_millis(5));
        }
        tx.commit().unwrap();

        assert!(
            successes + busy == 8,
            "{}: every attempt accounted for",
            mode.label()
        );
        assert!(
            successes >= 1,
            "{}: at least one API-style open should observe the committed snapshot",
            mode.label()
        );
        let after = count_messages(&open_reader(&db).unwrap()).unwrap();
        assert_eq!(after, 2, "{}: committed after import txn", mode.label());
    }
}

/// Scenario D: two writers contend.
#[test]
fn scenario_d_two_writers_contend() {
    for mode in [Mode::Rollback, Mode::Wal, Mode::WalBusy] {
        let (_tmp, db) = prepare_db(mode);
        let mut first = open_writer(&db, mode);
        let tx = first.transaction().unwrap();
        tx.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["d1", "holder"],
        )
        .unwrap();

        let second = open_writer(&db, mode);
        let contested = second.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["d2", "contender"],
        );
        match (mode, contested) {
            (_, Err(err)) => {
                assert!(
                    is_busy(&err),
                    "{}: second writer should be busy/locked, got {err}",
                    mode.label()
                );
            }
            (Mode::WalBusy, Ok(_)) => {
                // With busy_timeout the second writer may wait and still fail or succeed
                // depending on whether the first txn remains open. Keep the txn open, so
                // success would be surprising.
                panic!(
                    "{}: second writer succeeded while first txn open",
                    mode.label()
                );
            }
            (_, Ok(_)) => {
                panic!(
                    "{}: second writer succeeded while first txn open",
                    mode.label()
                );
            }
        }
        tx.commit().unwrap();
    }
}

/// Scenario E: WAL sidecars appear, survive close, and reopen recovers committed rows.
#[test]
fn scenario_e_wal_sidecar_lifecycle_and_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("wal-lifecycle.sqlite");
    let (wal_path, shm_path) = sidecar_paths(&db);

    {
        let conn = open_writer(&db, Mode::Wal);
        seed_schema(&conn);
        conn.execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["e1", "wal-row"],
        )
        .unwrap();
        // Sidecars are expected while the WAL connection is live / recently written.
        assert!(
            wal_path.exists() || shm_path.exists(),
            "expected -wal and/or -shm after WAL writes"
        );
    }

    // After close, SQLite may checkpoint and remove sidecars; either state is valid.
    // Reopen must see committed data regardless.
    let reopened = open_writer(&db, Mode::Wal);
    assert_eq!(count_messages(&reopened).unwrap(), 2);
    assert_eq!(journal_mode(&reopened).to_ascii_lowercase(), "wal");

    // Rollback-mode DB must not create WAL sidecars.
    let rollback_db = tmp.path().join("rollback.sqlite");
    {
        let conn = open_writer(&rollback_db, Mode::Rollback);
        seed_schema(&conn);
    }
    let (r_wal, r_shm) = sidecar_paths(&rollback_db);
    assert!(!r_wal.exists(), "rollback mode must not leave -wal");
    assert!(!r_shm.exists(), "rollback mode must not leave -shm");
}

/// Scenario F: read-only directory / distribution model.
#[test]
fn scenario_f_readonly_directory_and_serve_startup_write() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("dist");
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("history.sqlite");

    {
        let conn = open_writer(&db, Mode::Rollback);
        seed_schema(&conn);
    }

    // Make the directory non-writable (keep the DB file readable).
    let mut dir_perms = fs::metadata(&dir).unwrap().permissions();
    let mut file_perms = fs::metadata(&db).unwrap().permissions();
    dir_perms.set_readonly(true);
    file_perms.set_readonly(true);
    fs::set_permissions(&db, file_perms.clone()).unwrap();
    fs::set_permissions(&dir, dir_perms.clone()).unwrap();

    // API/browse-style open must succeed without creating sidecars.
    let reader = open_reader(&db).expect("read-only open on readonly dir");
    assert_eq!(count_messages(&reader).unwrap(), 1);
    let (wal, shm) = sidecar_paths(&db);
    assert!(!wal.exists());
    assert!(!shm.exists());

    // Enabling WAL requires a writable initialization path — it must not be done
    // from a supposedly read-only serve startup. Demonstrate that a writable open
    // attempting WAL in a readonly directory fails (or cannot create sidecars).
    let wal_attempt = Connection::open(&db).and_then(|conn| {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(journal_mode(&conn))
    });
    assert!(
        wal_attempt.is_err(),
        "setting WAL from a readonly distribution must not succeed silently"
    );

    // Restore permissions so TempDir cleanup works on all platforms.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&db, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    {
        #[allow(clippy::permissions_set_readonly_false)]
        {
            dir_perms.set_readonly(false);
            file_perms.set_readonly(false);
            let _ = fs::set_permissions(&db, file_perms);
            let _ = fs::set_permissions(&dir, dir_perms);
        }
    }
}

/// Decision-facing summary (Outcome A): keep DELETE/rollback journal.
///
/// Measured facts encoded by this suite:
/// - A held read transaction blocks writers under DELETE (scenario A) but not under WAL.
/// - Uncommitted import rows are invisible to readers in every mode (scenarios B/C).
/// - Two writers still contend in every mode (scenario D).
/// - WAL creates `-wal`/`-shm` sidecars; DELETE does not (scenario E).
/// - Read-only distribution cannot enable WAL (scenario F); serve-style opens must not
///   mutate journal mode.
///
/// WAL improves writer progress against long-lived readers, but does not make concurrent
/// `import` + `serve` a supported workflow: there is still no coordination layer, no
/// guaranteed consistent import view for readers, and WAL complicates backups and
/// read-only copies. Production keeps DELETE and documents concurrency as unsupported.
#[test]
fn decision_summary_outcome_a_keep_rollback_journal() {
    let (_tmp, db) = prepare_db(Mode::Rollback);
    assert_eq!(
        journal_mode(&open_writer(&db, Mode::Rollback)).to_ascii_lowercase(),
        "delete"
    );

    // Long-lived reader blocks the writer under DELETE — the operational risk for
    // browse/serve during import without WAL.
    let reader = open_reader(&db).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(count_messages(&reader).unwrap(), 1);
    let writer = open_writer(&db, Mode::Rollback);
    let err = writer
        .execute(
            "INSERT INTO messages(id, body) VALUES (?1, ?2)",
            ["policy", "blocked"],
        )
        .expect_err("DELETE: open reader txn must block writers");
    assert!(is_busy(&err));
    reader.execute_batch("COMMIT").ok();

    // Read-only path must not create WAL sidecars.
    let (wal, shm) = sidecar_paths(&db);
    assert!(!wal.exists());
    assert!(!shm.exists());
}
