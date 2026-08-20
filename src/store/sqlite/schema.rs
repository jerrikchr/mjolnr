//! Schema, pragmas, and migrations.
//!
//! One reason to change: the shape of the database.
//!
//! Everything here runs on `tokio-rusqlite`'s connection thread, so it is
//! ordinary blocking `rusqlite` code (`docs/persistence.md` §1.2).

use tokio_rusqlite::rusqlite::{Connection, Transaction};

/// The schema version this build writes and understands.
///
/// Stored in `PRAGMA user_version`, which SQLite persists in the file header and
/// never touches itself. Distinct from
/// [`WIRE_VERSION`](crate::store::wire::WIRE_VERSION): this describes the
/// tables, that describes the payloads inside them.
pub(super) const SCHEMA_VERSION: u32 = 5;

/// How long a writer waits for a competing writer before failing.
///
/// Finite on purpose. WAL still returns `SQLITE_BUSY` when two writers collide
/// (`docs/persistence.md` §2.1); an infinite wait would turn that into a hang
/// with no diagnostic, which is worse than an error.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Every migration, in order. The index is the version it produces.
///
/// A plain array rather than a migration framework: the entire feature is "run
/// the statements whose version exceeds `user_version`, in one transaction". A
/// crate for that would be more code to audit than the code it replaces, and
///  asks for a framework only if genuinely necessary.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        statements: INITIAL_SCHEMA,
    },
    Migration {
        version: 2,
        statements: SUBAGENT_LINK,
    },
    Migration {
        version: 3,
        statements: SESSION_TREE,
    },
    Migration {
        version: 4,
        statements: WORKSPACE_SEARCH,
    },
    Migration {
        version: 5,
        statements: WORKSPACE_SEARCH_COLUMNS,
    },
];

struct Migration {
    version: u32,
    statements: &'static str,
}

/// 's minimum schema, plus `session_owners`.
///
/// `session_owners` is the only addition, and it exists because
/// requires "no split-brain writer" while nothing in the §9 schema can express
/// ownership. See `docs/persistence.md` §5.
const INITIAL_SCHEMA: &str = "
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  root_realpath TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  last_opened_at TEXT NOT NULL
);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  active_provider TEXT,
  active_model TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_sequence INTEGER NOT NULL DEFAULT 0,
  last_checkpoint_sequence INTEGER
);

CREATE TABLE events (
  session_id TEXT NOT NULL REFERENCES sessions(id),
  sequence INTEGER NOT NULL,
  event_id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  PRIMARY KEY (session_id, sequence)
);

CREATE TABLE checkpoints (
  session_id TEXT NOT NULL REFERENCES sessions(id),
  sequence INTEGER NOT NULL,
  schema_version INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (session_id, sequence)
);

CREATE TABLE provider_profiles (
  id TEXT PRIMARY KEY,
  provider_kind TEXT NOT NULL,
  display_name TEXT NOT NULL,
  base_url TEXT,
  enabled INTEGER NOT NULL,
  metadata_json TEXT NOT NULL
);

CREATE TABLE session_owners (
  session_id TEXT PRIMARY KEY REFERENCES sessions(id),
  owner_token TEXT NOT NULL,
  process_id INTEGER NOT NULL,
  acquired_at TEXT NOT NULL
);

CREATE INDEX events_by_session_sequence ON events(session_id, sequence);
CREATE INDEX sessions_by_project ON sessions(project_id);
";

/// Phase 13: a child session records which session spawned it.
///
/// A real column rather than an event-scan because "the children of session X"
/// is a question `sessions list` and recovery both ask, and answering it by
/// replaying every transcript would make linkage a convention instead of a
/// fact the database enforces.
const SUBAGENT_LINK: &str = "
ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id);
CREATE INDEX sessions_by_parent ON sessions(parent_session_id);
";

/// Phase 16.5: a session is a tree of events, not only a line.
///
/// `parent_sequence` is the event this one followed. `NULL` means "the
/// immediately preceding sequence", which is exactly what every event written
/// before this migration meant — so every existing session reads as a
/// single-branch tree with no backfill and no rewrite. Only a branch point
/// writes a non-null parent that is not `sequence - 1`.
///
/// `active_leaf_sequence` on `sessions` records where the session currently
/// sits. `NULL` means the highest sequence, again matching pre-migration
/// behaviour: a linear session is always at its own tip.
const SESSION_TREE: &str = "
ALTER TABLE events ADD COLUMN parent_sequence INTEGER;
ALTER TABLE sessions ADD COLUMN active_leaf_sequence INTEGER;
CREATE INDEX events_by_parent ON events(session_id, parent_sequence);
";

/// Phase D4: Deterministic workspace search — schema only.
///
/// The D4 split landed the contract (this table) without a producer: nothing
/// wrote to or read from `workspace_search`. Superseded by
/// [`WORKSPACE_SEARCH_COLUMNS`](self::WORKSPACE_SEARCH_COLUMNS) in migration 5;
/// kept verbatim because a migration already applied to a database cannot be
/// edited — rewriting history here would give two installs different schemas
/// under the same `user_version`.
const WORKSPACE_SEARCH: &str = "
CREATE VIRTUAL TABLE workspace_search USING fts5(
  project_id UNINDEXED,
  session_id UNINDEXED,
  event_id UNINDEXED,
  event_kind UNINDEXED,
  status UNINDEXED,
  provider_model UNINDEXED,
  reason_code UNINDEXED,
  file_path UNINDEXED,
  time_range UNINDEXED,
  text_content,
  tokenize='trigram'
);
";

/// Phase D4 producer: the index declares exactly the columns it writes.
///
/// Two changes from migration 4, and the second is the reason this migration
/// exists at all rather than being deferred as tidying:
///
/// 1. **`status` and `time_range` are gone.** The producer never wrote them,
///    deliberately: a session's status changes, and a copy of it inside an
///    append-only index is stale from the moment it does. Both facts are joined
///    back from `sessions` and `events`, where they are authoritative (standing
///    law 5). Columns a producer refuses to fill on principle should not be in
///    the schema claiming they exist.
/// 2. **`text_content` is now the last column, and the only indexed one.**
///    FTS5's `snippet()` addresses columns *positionally*, and in migration 4
///    the index the producer passed pointed at `file_path` — so every snippet
///    was drawn from the wrong column. Removing the dead columns shortens the
///    distance between the schema and the query, and
///    `SNIPPET_TEXT_COLUMN` in `search.rs` now names the position instead of
///    spelling a number nobody can check by eye.
///
/// Dropping and recreating rather than migrating rows: FTS5 has no `ALTER`, and
/// the table is a *projection* — `events` is the durable record, so throwing the
/// index away costs nothing that cannot be rebuilt. It is left empty on purpose;
/// `SqliteEventStore::rebuild_search_index` repopulates it, and an empty index
/// that a caller can rebuild is safer than a half-migrated one that silently
/// answers with pre-migration rows.
const WORKSPACE_SEARCH_COLUMNS: &str = "
DROP TABLE IF EXISTS workspace_search;
CREATE VIRTUAL TABLE workspace_search USING fts5(
  event_id UNINDEXED,
  session_id UNINDEXED,
  project_id UNINDEXED,
  event_kind UNINDEXED,
  provider_model UNINDEXED,
  reason_code UNINDEXED,
  file_path UNINDEXED,
  text_content,
  tokenize='trigram'
);
";

/// Apply per-connection settings.
///
/// Both of these are per-connection and neither is persisted, so this runs on
/// every open — including reopens of an already-migrated database. Forgetting
/// `foreign_keys` would leave every `REFERENCES` in the schema decorative
/// (`docs/persistence.md` §2.1).
pub(super) fn apply_connection_pragmas(
    connection: &Connection,
) -> Result<(), tokio_rusqlite::rusqlite::Error> {
    // Must precede any transaction: `foreign_keys` is a no-op inside one.
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
    Ok(())
}

/// Bring a database up to [`SCHEMA_VERSION`], returning the version found.
///
/// # Errors
/// Propagates SQLite failures. A version newer than this build is *not* an error
/// here — [`MigrationOutcome`] reports it so the caller can produce the typed
/// refusal, since `core` owns that error vocabulary and this module must not.
pub(super) fn migrate(
    connection: &mut Connection,
) -> Result<MigrationOutcome, tokio_rusqlite::rusqlite::Error> {
    // Read the compatibility gate before any persistent pragma or schema
    // change. `journal_mode = WAL` modifies the database file, so issuing it
    // before this check would mutate a newer database even though mjolnr then
    // refused to open it.
    let found = user_version(connection)?;
    if found > SCHEMA_VERSION {
        return Ok(MigrationOutcome::TooNew { found });
    }

    // WAL is persistent in the file and cannot be set inside a transaction, so
    // it is issued once, here, outside the migration transaction.
    connection.pragma_update(None, "journal_mode", "WAL")?;

    for migration in MIGRATIONS {
        if migration.version <= found {
            continue;
        }
        let transaction = connection.transaction()?;
        apply(&transaction, migration)?;
        transaction.commit()?;
    }

    Ok(MigrationOutcome::Ready { from: found })
}

/// What [`migrate`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MigrationOutcome {
    /// The database is at [`SCHEMA_VERSION`]. `from` is the version it started
    /// at — equal to the target when nothing was applied.
    Ready { from: u32 },
    /// Written by a newer mjolnr. The caller refuses.
    TooNew { found: u32 },
}

/// Apply one migration and stamp its version, in the caller's transaction.
///
/// The stamp is inside the same transaction as the statements, so a failure
/// halfway leaves neither. There is no half-migrated state to diagnose.
fn apply(
    transaction: &Transaction<'_>,
    migration: &Migration,
) -> Result<(), tokio_rusqlite::rusqlite::Error> {
    transaction.execute_batch(migration.statements)?;
    // `pragma_update` does not accept a bound parameter for user_version, and
    // the value is a compile-time constant from MIGRATIONS, not user input.
    transaction.pragma_update(None, "user_version", migration.version)?;
    Ok(())
}

pub(super) fn user_version(
    connection: &Connection,
) -> Result<u32, tokio_rusqlite::rusqlite::Error> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map(|version| u32::try_from(version).unwrap_or(u32::MAX))
}

/// Read a per-connection or persistent pragma as text, for diagnostics.
pub(super) fn pragma_text(
    connection: &Connection,
    name: &str,
) -> Result<String, tokio_rusqlite::rusqlite::Error> {
    connection.pragma_query_value(None, name, |row| row.get::<_, String>(0))
}

/// Read a numeric pragma, for diagnostics.
pub(super) fn pragma_number(
    connection: &Connection,
    name: &str,
) -> Result<i64, tokio_rusqlite::rusqlite::Error> {
    connection.pragma_query_value(None, name, |row| row.get::<_, i64>(0))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "AGENTS.md §7: tests may panic freely")]
mod tests {
    use super::*;

    fn migrated() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        apply_connection_pragmas(&connection).unwrap();
        migrate(&mut connection).unwrap();
        connection
    }

    #[test]
    fn an_empty_database_migrates_to_the_current_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply_connection_pragmas(&connection).unwrap();

        assert_eq!(user_version(&connection).unwrap(), 0);
        let outcome = migrate(&mut connection).unwrap();

        assert_eq!(outcome, MigrationOutcome::Ready { from: 0 });
        assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply_connection_pragmas(&connection).unwrap();

        migrate(&mut connection).unwrap();
        // The second run must apply nothing. If it re-ran the batch, CREATE
        // TABLE would fail — which is exactly the assertion.
        let outcome = migrate(&mut connection).unwrap();

        assert_eq!(
            outcome,
            MigrationOutcome::Ready {
                from: SCHEMA_VERSION
            },
            "an up-to-date database must apply no migration"
        );
        assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_is_reported_rather_than_migrated() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply_connection_pragmas(&connection).unwrap();
        migrate(&mut connection).unwrap();

        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .unwrap();

        assert_eq!(
            migrate(&mut connection).unwrap(),
            MigrationOutcome::TooNew {
                found: SCHEMA_VERSION + 5
            },
            "a database from a newer mjolnr must not be touched"
        );
    }

    #[test]
    fn every_table_from_the_plan_exists() {
        let connection = migrated();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        for required in [
            "checkpoints",
            "events",
            "projects",
            "provider_profiles",
            "session_owners",
            "sessions",
        ] {
            assert!(
                tables.iter().any(|name| name == required),
                " requires a `{required}` table; found {tables:?}"
            );
        }
    }

    #[test]
    fn version_one_databases_gain_the_parent_link() {
        // Reproduce a database created by a pre-Phase-13 build, then migrate.
        let mut connection = Connection::open_in_memory().unwrap();
        apply_connection_pragmas(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(INITIAL_SCHEMA).unwrap();
        transaction.pragma_update(None, "user_version", 1).unwrap();
        transaction.commit().unwrap();

        assert_eq!(
            migrate(&mut connection).unwrap(),
            MigrationOutcome::Ready { from: 1 }
        );

        let parent_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'parent_session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_column, 1, "sessions must carry parent_session_id");
    }

    #[test]
    fn foreign_keys_are_enforced_on_the_connection() {
        // The failure this catches is silent: without the pragma every
        // REFERENCES clause above is decoration, and orphaned events accumulate
        // until a join returns nothing.
        let connection = migrated();
        assert_eq!(pragma_number(&connection, "foreign_keys").unwrap(), 1);

        let orphan = connection.execute(
            "INSERT INTO events (session_id, sequence, event_id, kind, occurred_at, schema_version, payload_json)
             VALUES ('no-such-session', 0, 'e1', 'run_started', '2026-01-01T00:00:00Z', 1, '{}')",
            [],
        );
        assert!(
            orphan.is_err(),
            "an event referencing no session must be refused by the database"
        );
    }

    #[test]
    fn a_finite_busy_timeout_is_set() {
        let connection = migrated();
        assert_eq!(
            pragma_number(&connection, "busy_timeout").unwrap(),
            i64::from(BUSY_TIMEOUT_MS)
        );
    }

    #[test]
    fn duplicate_event_ids_are_refused_by_the_database() {
        // `finish_task` cites event ids as evidence. Two events sharing one id
        // would make evidence ambiguous, so the constraint is in the schema
        // rather than in a check some future writer might skip.
        let connection = migrated();
        seed_session(&connection);

        insert_event(&connection, 0, "shared-id").unwrap();
        let duplicate = insert_event(&connection, 1, "shared-id");

        assert!(duplicate.is_err(), "a duplicate event id must be refused");
    }

    #[test]
    fn a_duplicate_sequence_is_refused_by_the_database() {
        let connection = migrated();
        seed_session(&connection);

        insert_event(&connection, 0, "e0").unwrap();
        let duplicate = insert_event(&connection, 0, "e1");

        assert!(
            duplicate.is_err(),
            "two events cannot occupy one sequence slot"
        );
    }

    fn seed_session(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO projects (id, root_realpath, created_at, last_opened_at)
                 VALUES ('p1', '/tmp/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions (id, project_id, title, status, created_at, updated_at)
                 VALUES ('s1', 'p1', 't', 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
    }

    fn insert_event(
        connection: &Connection,
        sequence: i64,
        event_id: &str,
    ) -> Result<usize, tokio_rusqlite::rusqlite::Error> {
        connection.execute(
            "INSERT INTO events (session_id, sequence, event_id, kind, occurred_at, schema_version, payload_json)
             VALUES ('s1', ?1, ?2, 'run_started', '2026-01-01T00:00:00Z', 1, '{}')",
            tokio_rusqlite::rusqlite::params![sequence, event_id],
        )
    }
}
