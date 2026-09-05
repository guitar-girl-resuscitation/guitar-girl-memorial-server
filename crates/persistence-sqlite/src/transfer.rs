//! Offline save transfer. Callers must exclude runtime startup for the entire
//! operation. SQLite's backup transaction includes committed WAL pages and
//! rolls back an incomplete restore; we never create a backup of the old save.
use std::{fs, path::Path};
use rusqlite::{Connection, OpenFlags, backup::{Backup, StepResult}};
use crate::{Database, StoreError, DATABASE_SCHEMA_VERSION};

pub const MAX_SAVE_BYTES: u64 = 256 * 1024 * 1024;

fn schema(db: &Connection) -> Result<Vec<(String, String, Option<String>)>, StoreError> {
    Ok(db.prepare("SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?)
}

fn validated(path: &Path) -> Result<Connection, StoreError> {
    let size = fs::metadata(path)?.len();
    if size == 0 || size > MAX_SAVE_BYTES {
        return Err(StoreError::Integrity("save size outside supported limits".into()));
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != DATABASE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema { actual: version, expected: DATABASE_SCHEMA_VERSION });
    }
    let expected = Database::open_memory()?;
    if schema(&db)? != schema(&expected.connection)? {
        return Err(StoreError::Integrity("save schema does not match this build".into()));
    }
    let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if check != "ok" || db.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(StoreError::Integrity("save integrity or foreign key check failed".into()));
    }
    Ok(db)
}

fn copy_transaction(source: &Connection, destination: &mut Connection) -> Result<(), StoreError> {
    destination.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
    let copy = Backup::new(source, destination)?;
    // One SQLite transaction; unfinished copies are rolled back on drop.
    match copy.step(-1)? {
        StepResult::Done => Ok(()),
        _ => Err(StoreError::Integrity("save is busy; retry before starting the game".into())),
    }
}

pub fn export(source: &Path, output: &Path) -> Result<(), StoreError> {
    let source = validated(source)?;
    // Do not overwrite an existing export or follow an existing symlink.
    let reserved = fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    drop(reserved);
    let mut destination = Connection::open(output)?;
    copy_transaction(&source, &mut destination)?;
    // Export one standalone file, with no journal/WAL companion required.
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    drop(destination);
    fs::OpenOptions::new().write(true).open(output)?.sync_all()?;
    Ok(())
}

pub fn import(source: &Path, destination: &Path) -> Result<(), StoreError> {
    // Validate before touching the current database, never create/migrate the input.
    let source_db = validated(source)?;
    if destination.exists() && fs::canonicalize(source)? == fs::canonicalize(destination)? {
        return Err(StoreError::Integrity("source and destination must differ".into()));
    }
    let mut current = Connection::open(destination)?;
    copy_transaction(&source_db, &mut current)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_wal_and_restore_all_slots_without_old_save_backup() {
        let root = std::env::temp_dir().join(format!("ggfm-transfer-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.sqlite3");
        let snapshot = root.join("export.sqlite3");
        let current = root.join("current.sqlite3");
        let invalid = root.join("invalid.sqlite3");
        let mut db = Database::open(&source).unwrap();
        db.create_slot("first", 1).unwrap();
        db.create_slot("second", 2).unwrap();
        export(&source, &snapshot).unwrap(); // source WAL connection deliberately remains open
        assert!(export(&source, &snapshot).is_err());
        drop(db);
        let mut old = Database::open(&current).unwrap();
        old.create_slot("old", 0).unwrap();
        drop(old);
        fs::write(&invalid, b"not a save").unwrap();
        assert!(import(&invalid, &current).is_err());
        let before = Connection::open(&current).unwrap();
        assert_eq!(before.query_row("SELECT count(*) FROM save_slots", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        drop(before);
        import(&snapshot, &current).unwrap();
        let after = validated(&current).unwrap();
        assert_eq!(after.query_row("SELECT count(*) FROM save_slots", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert!(import(&current, &current).is_err());
        drop(after);
        assert!(!fs::read_dir(&root).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("backup")));
        for file in [source, snapshot, current, invalid] {
            for suffix in ["-wal", "-shm"] {
                let sidecar = std::path::PathBuf::from(format!("{}{suffix}", file.display()));
                if sidecar.is_file() { fs::remove_file(sidecar).unwrap(); }
            }
            fs::remove_file(file).unwrap();
        }
        // SQLite sidecars have been closed above; remove only this empty test directory.
        fs::remove_dir(root).unwrap();
    }
}
