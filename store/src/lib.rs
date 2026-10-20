use rusqlite::{Connection, OpenFlags, params};
use std::path::PathBuf;

pub use rusqlite::Result;

/// The collector records at least this often while the machine is awake, so a
/// longer silence between two samples means it was suspended or powered off.
pub const MAX_AWAKE_GAP_SECONDS: i64 = 150;

/// One extra day beyond the seven shown, so the oldest visible day is complete.
pub const RETENTION_SECONDS: i64 = 8 * 24 * 3600;

/// The state of the machine at `ts`. `plugged` holds from this sample until the
/// next one; `screen_on` describes the stretch that ended at this sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub ts: i64,
    pub battery_percent: u8,
    pub plugged: bool,
    pub screen_on: bool,
}

/// How long an app counted as in use during the stretch ending at a sample.
pub struct AppSeconds {
    pub app: String,
    pub seconds: i64,
    pub focused_seconds: i64,
}

pub struct AppTotal {
    pub app: String,
    pub window_class: Option<String>,
    pub seconds: i64,
    pub focused_seconds: i64,
}

pub fn database_path() -> PathBuf {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data_home.join("joule/joule.db")
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open_for_recording() -> Result<Self> {
        let path = database_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| rusqlite::Error::InvalidPath(dir.to_path_buf()))?;
        }
        let conn = Connection::open(path)?;
        // WAL lets the app read while the collector is writing.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS samples (
                 ts INTEGER PRIMARY KEY,
                 battery_percent INTEGER NOT NULL,
                 plugged INTEGER NOT NULL,
                 screen_on INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS app_usage (
                 ts INTEGER NOT NULL,
                 app TEXT NOT NULL,
                 seconds INTEGER NOT NULL,
                 focused_seconds INTEGER NOT NULL,
                 PRIMARY KEY (ts, app)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS app_window_classes (
                 app TEXT PRIMARY KEY,
                 window_class TEXT NOT NULL
             ) WITHOUT ROWID;",
        )?;
        Ok(Self { conn })
    }

    pub fn open_read_only() -> Result<Self> {
        let conn = Connection::open_with_flags(database_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn })
    }

    pub fn record(&mut self, sample: &Sample, apps: &[AppSeconds]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO samples (ts, battery_percent, plugged, screen_on) VALUES (?1, ?2, ?3, ?4)",
            params![sample.ts, sample.battery_percent, sample.plugged, sample.screen_on],
        )?;
        for usage in apps {
            tx.execute(
                "INSERT OR REPLACE INTO app_usage (ts, app, seconds, focused_seconds) VALUES (?1, ?2, ?3, ?4)",
                params![sample.ts, usage.app, usage.seconds, usage.focused_seconds],
            )?;
        }
        tx.commit()
    }

    pub fn remember_window_class(&self, app: &str, window_class: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO app_window_classes (app, window_class) VALUES (?1, ?2)",
            params![app, window_class],
        )?;
        Ok(())
    }

    pub fn delete_older_than(&self, ts: i64) -> Result<()> {
        self.conn.execute("DELETE FROM samples WHERE ts < ?1", [ts])?;
        self.conn.execute("DELETE FROM app_usage WHERE ts < ?1", [ts])?;
        Ok(())
    }

    pub fn samples_since(&self, ts: i64) -> Result<Vec<Sample>> {
        let mut statement = self
            .conn
            .prepare("SELECT ts, battery_percent, plugged, screen_on FROM samples WHERE ts >= ?1 ORDER BY ts")?;
        let rows = statement.query_map([ts], |row| {
            Ok(Sample {
                ts: row.get(0)?,
                battery_percent: row.get(1)?,
                plugged: row.get(2)?,
                screen_on: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Per-app time for samples in `(after, until]`, most used first.
    pub fn app_totals(&self, after: i64, until: i64) -> Result<Vec<AppTotal>> {
        let mut statement = self.conn.prepare(
            "SELECT u.app, c.window_class, SUM(u.seconds), SUM(u.focused_seconds)
             FROM app_usage u LEFT JOIN app_window_classes c ON c.app = u.app
             WHERE u.ts > ?1 AND u.ts <= ?2
             GROUP BY u.app
             ORDER BY 3 DESC, 4 DESC",
        )?;
        let rows = statement.query_map([after, until], |row| {
            Ok(AppTotal {
                app: row.get(0)?,
                window_class: row.get(1)?,
                seconds: row.get(2)?,
                focused_seconds: row.get(3)?,
            })
        })?;
        rows.collect()
    }
}
