//! Saved VPN connection profiles, persisted in a local SQLite DB
//! (%APPDATA%\yellow-vpn\profiles.db). Lives entirely in the unprivileged GUI
//! process; the elevated helper never sees this DB. Passwords are stored in
//! plaintext per the product decision (see the design's risk note).
use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub protocol: String,
    pub insecure: bool,
    pub cert_sha256: Option<String>,
    /// FortiGate authentication realm; `None`/empty means the default realm.
    #[serde(default)]
    pub realm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProfile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub protocol: String,
    pub insecure: bool,
    pub cert_sha256: Option<String>,
    #[serde(default)]
    pub realm: Option<String>,
}

/// Managed-state wrapper around the SQLite connection.
pub struct Db(pub Mutex<Connection>);

impl Db {
    /// Open (or create) the DB at `path` and ensure the schema exists.
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Db(Mutex::new(conn)))
    }

    fn init(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 443,
                username TEXT NOT NULL,
                password TEXT NOT NULL,
                protocol TEXT NOT NULL,
                insecure INTEGER NOT NULL DEFAULT 0,
                cert_sha256 TEXT
             );",
        )?;
        // Migration: `realm` (FortiGate) was added after the first release, so an
        // existing DB has no such column. SQLite has no `ADD COLUMN IF NOT
        // EXISTS`, and re-adding one is a hard error — check first.
        if !Self::has_column(conn, "profiles", "realm")? {
            conn.execute_batch("ALTER TABLE profiles ADD COLUMN realm TEXT;")?;
        }
        Ok(())
    }

    /// Whether `table` already has `column`, via `PRAGMA table_info`.
    fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if row.get::<_, String>(1)? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Profile>> {
        let conn = self.0.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id,name,host,port,username,password,protocol,insecure,cert_sha256,realm
             FROM profiles ORDER BY name",
        )?;
        let rows = stmt.query_map([], Self::row_to_profile)?;
        rows.collect()
    }

    pub fn create(&self, p: &NewProfile) -> rusqlite::Result<Profile> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO profiles (name,host,port,username,password,protocol,insecure,cert_sha256,realm)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                p.name,
                p.host,
                p.port,
                p.username,
                p.password,
                p.protocol,
                p.insecure as i64,
                p.cert_sha256,
                p.realm
            ],
        )?;
        let id = conn.last_insert_rowid();
        Self::get(&conn, id)
    }

    pub fn update(&self, id: i64, p: &NewProfile) -> rusqlite::Result<Profile> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE profiles SET name=?1,host=?2,port=?3,username=?4,password=?5,protocol=?6,insecure=?7,cert_sha256=?8,realm=?9
             WHERE id=?10",
            rusqlite::params![
                p.name,
                p.host,
                p.port,
                p.username,
                p.password,
                p.protocol,
                p.insecure as i64,
                p.cert_sha256,
                p.realm,
                id
            ],
        )?;
        Self::get(&conn, id)
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM profiles WHERE id=?1", [id])?;
        Ok(())
    }

    fn get(conn: &Connection, id: i64) -> rusqlite::Result<Profile> {
        conn.query_row(
            "SELECT id,name,host,port,username,password,protocol,insecure,cert_sha256,realm
             FROM profiles WHERE id=?1",
            [id],
            Self::row_to_profile,
        )
    }

    fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<Profile> {
        Ok(Profile {
            id: row.get(0)?,
            name: row.get(1)?,
            host: row.get(2)?,
            port: row.get(3)?,
            username: row.get(4)?,
            password: row.get(5)?,
            protocol: row.get(6)?,
            insecure: row.get::<_, i64>(7)? != 0,
            cert_sha256: row.get(8)?,
            realm: row.get(9)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        Db::init(&conn).unwrap();
        Db(Mutex::new(conn))
    }

    fn sample() -> NewProfile {
        NewProfile {
            name: "work".into(),
            host: "vpn.example.com".into(),
            port: 443,
            username: "alice".into(),
            password: "s3cret".into(),
            protocol: "Checkpoint".into(),
            insecure: true,
            cert_sha256: None,
            realm: None,
        }
    }

    #[test]
    fn create_then_list_round_trips() {
        let db = mem_db();
        let created = db.create(&sample()).unwrap();
        assert!(created.id > 0);
        let all = db.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "work");
        assert!(all[0].insecure); // int 1 -> bool true
        assert_eq!(all[0].cert_sha256, None);
        assert_eq!(all[0].port, 443);
    }

    #[test]
    fn update_mutates() {
        let db = mem_db();
        let c = db.create(&sample()).unwrap();
        let mut np = sample();
        np.name = "work-edited".into();
        np.insecure = false;
        np.cert_sha256 = Some("aa:bb".into());
        let u = db.update(c.id, &np).unwrap();
        assert_eq!(u.name, "work-edited");
        assert!(!u.insecure);
        assert_eq!(u.cert_sha256, Some("aa:bb".into()));
    }

    #[test]
    fn delete_removes() {
        let db = mem_db();
        let c = db.create(&sample()).unwrap();
        db.delete(c.id).unwrap();
        assert!(db.list().unwrap().is_empty());
    }

    #[test]
    fn realm_round_trips() {
        let db = mem_db();
        let mut np = sample();
        np.realm = Some("corp".into());
        let c = db.create(&np).unwrap();
        assert_eq!(c.realm.as_deref(), Some("corp"));
        np.realm = None;
        assert_eq!(db.update(c.id, &np).unwrap().realm, None);
    }

    #[test]
    fn migrates_a_pre_realm_database() {
        // Exactly the schema shipped before `realm` existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL, host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 443,
                username TEXT NOT NULL, password TEXT NOT NULL,
                protocol TEXT NOT NULL,
                insecure INTEGER NOT NULL DEFAULT 0,
                cert_sha256 TEXT);
             INSERT INTO profiles (name,host,port,username,password,protocol,insecure)
             VALUES ('old','h',443,'u','p','FortiGate',0);",
        )
        .unwrap();

        Db::init(&conn).unwrap();
        let db = Db(Mutex::new(conn));
        let all = db.list().unwrap();
        assert_eq!(all.len(), 1, "the existing row must survive the migration");
        assert_eq!(all[0].name, "old");
        assert_eq!(all[0].realm, None);

        // Running init again must not try to re-add the column.
        Db::init(&db.0.lock().unwrap()).unwrap();
        assert_eq!(db.list().unwrap().len(), 1);
    }
}
