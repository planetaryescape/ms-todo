// Pool setup adapted from mxr crates/store/src/pool.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// (one writer connection, a few read-only readers, WAL, busy timeout).
// Changes: sqlx's own embedded migrator instead of mxr's hand-rolled one,
// since ms-todo starts with a clean schema history.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::StoreError;

/// How long SQLite waits for the write lock before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// WAL readers don't block each other or the writer.
const READERS: u32 = 4;

pub struct Store {
    /// One connection: SQLite has one writer anyway, and a single
    /// connection means a write transaction never has to upgrade a read.
    writer: SqlitePool,
    reader: SqlitePool,
    path: PathBuf,
}

impl Store {
    /// Open (creating if needed) the database at `path` and run migrations.
    /// The cache holds the user's tasks, so the database and its WAL and
    /// shared-memory files are 0600, and existing ones are repaired. The
    /// caller keeps the directory private (the daemon makes it 0700).
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|error| {
                    StoreError::Invalid(format!("cannot create {}: {error}", dir.display()))
                })?;
        }
        // Created 0600 before SQLite opens it: SQLite gives the -wal and
        // -shm files the database file's mode.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| describe(path, &error))?;
        make_private(path)?;
        let url = format!("sqlite:{}", path.display());
        let base = SqliteConnectOptions::from_str(&url)?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(BUSY_TIMEOUT)
            .foreign_keys(true);
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone().create_if_missing(true))
            .await?;
        match sqlx::migrate!("./migrations").run(&writer).await {
            Ok(()) => {}
            // Applied, but not among this build's migrations.
            Err(sqlx::migrate::MigrateError::VersionMissing(_)) => {
                return Err(StoreError::NewerDatabase);
            }
            Err(error) => return Err(error.into()),
        }
        crate::search::fill_body_text(&writer).await?;
        let reader = SqlitePoolOptions::new()
            .max_connections(READERS)
            .connect_with(base.read_only(true))
            .await?;
        // Repairs -wal and -shm files an older build left world-readable.
        make_private(path)?;
        Ok(Self {
            writer,
            reader,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The database's size on disk, its WAL and shared-memory files included.
    pub fn size_bytes(&self) -> u64 {
        database_files(&self.path)
            .filter_map(|file| std::fs::metadata(file).ok())
            .map(|meta| meta.len())
            .sum()
    }

    pub(crate) fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    pub(crate) fn reader(&self) -> &SqlitePool {
        &self.reader
    }

    /// The daemon's own-write counter now. A sync pass reads it before it
    /// fetches, and leaves alone every row written after.
    pub async fn local_rev(&self) -> Result<i64, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT value FROM counters WHERE name = 'local_rev'")
                .fetch_one(&self.reader)
                .await?,
        )
    }
}

/// The database file and the WAL and shared-memory files beside it.
fn database_files(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    ["", "-wal", "-shm"].into_iter().map(move |suffix| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    })
}

/// Make each of the database's files that exists 0600.
fn make_private(path: &Path) -> Result<(), StoreError> {
    for file in database_files(path) {
        match std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(describe(&file, &error)),
        }
    }
    Ok(())
}

fn describe(path: &Path, error: &std::io::Error) -> StoreError {
    StoreError::Invalid(format!("{}: {error}", path.display()))
}

/// Bump the own-write counter inside `tx` and return the new value.
pub(crate) async fn next_local_rev(tx: &mut sqlx::SqliteConnection) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(
        "UPDATE counters SET value = value + 1 WHERE name = 'local_rev' RETURNING value",
    )
    .fetch_one(tx)
    .await?)
}
