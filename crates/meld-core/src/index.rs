use std::collections::HashSet;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::ChunkProfile;
use crate::chunker::visit_file_chunks;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexReport {
    pub source: PathBuf,
    pub database: PathBuf,
    pub profile: ChunkProfile,
    pub files_indexed: u64,
    pub bytes_indexed: u64,
    pub chunks_indexed: u64,
    pub database_bytes: u64,
    pub index_overhead_ratio: f64,
    pub elapsed_ms: u128,
    pub bytes_per_second: f64,
    pub skipped_entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexStats {
    pub files: u64,
    pub bytes: u64,
    pub chunks: u64,
    pub database_bytes: u64,
}

pub struct IndexDb {
    connection: Connection,
    path: PathBuf,
}

impl IndexDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open SQLite index {}", path.display()))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS chunks (
                 hash BLOB NOT NULL,
                 length INTEGER NOT NULL,
                 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 offset INTEGER NOT NULL,
                 PRIMARY KEY(hash, length, file_id, offset)
             );
             CREATE INDEX IF NOT EXISTS chunks_lookup ON chunks(hash, length);",
        )?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub fn index_directory(&mut self, source: &Path, profile: ChunkProfile) -> Result<IndexReport> {
        profile.validate()?;
        let source = source
            .canonicalize()
            .with_context(|| format!("canonicalize source directory {}", source.display()))?;
        if !source.is_dir() {
            bail!("index source is not a directory: {}", source.display());
        }

        let started = Instant::now();
        let database_absolute = absolute_path(&self.path)?;
        let mut files = Vec::new();
        let mut skipped_entries = Vec::new();
        for entry in WalkDir::new(&source).follow_links(false) {
            match entry {
                Ok(entry) if entry.file_type().is_file() => {
                    let path = entry.into_path();
                    let absolute = absolute_path(&path)?;
                    if is_database_sidecar(&absolute, &database_absolute) {
                        continue;
                    }
                    files.push(absolute);
                }
                Ok(_) => {}
                Err(error) => skipped_entries.push(error.to_string()),
            }
        }
        files.sort();
        let discovered: HashSet<PathBuf> = files.iter().cloned().collect();

        self.store_profile(profile)?;
        let mut files_indexed = 0_u64;
        let mut bytes_indexed = 0_u64;
        let mut chunks_indexed = 0_u64;
        for path in files {
            match self.index_file(&path, profile) {
                Ok((bytes, chunks)) => {
                    files_indexed += 1;
                    bytes_indexed += bytes;
                    chunks_indexed += chunks;
                }
                Err(error) => skipped_entries.push(format!("{}: {error:#}", path.display())),
            }
        }
        self.prune_missing_files(&source, &discovered)?;

        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let elapsed = started.elapsed();
        let database_bytes = database_disk_bytes(&self.path)?;
        let seconds = elapsed.as_secs_f64();
        Ok(IndexReport {
            source,
            database: self.path.clone(),
            profile,
            files_indexed,
            bytes_indexed,
            chunks_indexed,
            database_bytes,
            index_overhead_ratio: if bytes_indexed == 0 {
                0.0
            } else {
                database_bytes as f64 / bytes_indexed as f64
            },
            elapsed_ms: elapsed.as_millis(),
            bytes_per_second: if seconds == 0.0 {
                0.0
            } else {
                bytes_indexed as f64 / seconds
            },
            skipped_entries,
        })
    }

    fn store_profile(&self, profile: ChunkProfile) -> Result<()> {
        let serialized = serde_json::to_string(&profile)?;
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='chunk_profile'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != serialized {
                bail!(
                    "index was created with chunk profile {existing}; use a separate database for {serialized}"
                );
            }
        } else {
            self.connection.execute(
                "INSERT INTO meta(key, value) VALUES('chunk_profile', ?1)",
                [serialized],
            )?;
        }
        Ok(())
    }

    pub fn ensure_profile(&self, profile: ChunkProfile) -> Result<()> {
        profile.validate()?;
        let expected = serde_json::to_string(&profile)?;
        let actual: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='chunk_profile'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match actual {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => {
                bail!("descriptor profile {expected} does not match index profile {actual}")
            }
            None => bail!("index has no chunk profile; run index first"),
        }
    }

    fn index_file(&mut self, path: &Path, profile: ChunkProfile) -> Result<(u64, u64)> {
        let before = std::fs::metadata(path)
            .with_context(|| format!("read metadata before indexing {}", path.display()))?;
        if !before.is_file() {
            bail!("not a regular file");
        }
        let path_text = path
            .to_str()
            .context("non-UTF-8 paths are not supported in prototype 0.1")?;
        let size = before.len();
        let mtime_ns = modified_ns(&before)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO files(path, size, mtime_ns) VALUES(?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime_ns=excluded.mtime_ns",
            params![path_text, to_i64(size, "file size")?, mtime_ns],
        )?;
        let file_id: i64 =
            transaction.query_row("SELECT id FROM files WHERE path=?1", [path_text], |row| {
                row.get(0)
            })?;
        transaction.execute("DELETE FROM chunks WHERE file_id=?1", [file_id])?;

        let mut statement = transaction
            .prepare("INSERT INTO chunks(hash, length, file_id, offset) VALUES(?1, ?2, ?3, ?4)")?;
        let chunk_count = visit_file_chunks(path, profile, |chunk| {
            let hash = hex::decode(&chunk.hash)?;
            statement.execute(params![
                hash,
                i64::from(chunk.length),
                file_id,
                to_i64(chunk.offset, "chunk offset")?
            ])?;
            Ok(())
        })?;
        drop(statement);

        let after = std::fs::metadata(path)
            .with_context(|| format!("read metadata after indexing {}", path.display()))?;
        if after.len() != size || modified_ns(&after)? != mtime_ns {
            bail!("source changed while it was being indexed");
        }
        transaction.commit()?;
        Ok((size, chunk_count))
    }

    fn prune_missing_files(&self, source: &Path, discovered: &HashSet<PathBuf>) -> Result<()> {
        let mut statement = self.connection.prepare("SELECT id, path FROM files")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut stale_ids = Vec::new();
        for row in rows {
            let (id, path) = row?;
            let path = PathBuf::from(path);
            if path.starts_with(source) && !discovered.contains(&path) {
                stale_ids.push(id);
            }
        }
        drop(statement);
        for id in stale_ids {
            self.connection
                .execute("DELETE FROM files WHERE id=?1", [id])?;
        }
        Ok(())
    }

    pub fn lookup_chunk(&self, hash: &str, length: u32) -> Result<Option<SourceLocation>> {
        let hash_bytes = hex::decode(hash).context("decode requested chunk hash")?;
        let mut statement = self.connection.prepare(
            "SELECT files.path, files.size, files.mtime_ns, chunks.offset
             FROM chunks JOIN files ON files.id=chunks.file_id
             WHERE chunks.hash=?1 AND chunks.length=?2
             ORDER BY files.id, chunks.offset",
        )?;
        let rows = statement.query_map(params![hash_bytes, i64::from(length)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (path, expected_size, expected_mtime, offset) = row?;
            let path = PathBuf::from(path);
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.len() != expected_size as u64 || modified_ns(&metadata)? != expected_mtime {
                continue;
            }
            return Ok(Some(SourceLocation {
                path,
                offset: offset as u64,
                length,
            }));
        }
        Ok(None)
    }

    pub fn stats(&self) -> Result<IndexStats> {
        let (files, bytes): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM files",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let chunks: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(IndexStats {
            files: files as u64,
            bytes: bytes as u64,
            chunks: chunks as u64,
            database_bytes: database_disk_bytes(&self.path)?,
        })
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .with_context(|| format!("canonicalize parent {}", parent.display()))?;
    let name = path.file_name().context("path has no file name")?;
    Ok(parent.join(name))
}

fn is_database_sidecar(path: &Path, database: &Path) -> bool {
    if path == database {
        return true;
    }
    let database_text = database.to_string_lossy();
    let path_text = path.to_string_lossy();
    path_text == format!("{database_text}-wal") || path_text == format!("{database_text}-shm")
}

fn modified_ns(metadata: &Metadata) -> Result<i64> {
    let duration = metadata
        .modified()
        .context("source has no modification timestamp")?
        .duration_since(UNIX_EPOCH)
        .context("source modification timestamp is before UNIX epoch")?;
    i64::try_from(duration.as_nanos()).context("modification timestamp does not fit in SQLite")
}

fn to_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{label} does not fit in SQLite"))
}

fn database_disk_bytes(path: &Path) -> Result<u64> {
    let mut total = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        total += std::fs::metadata(sidecar)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    }
    Ok(total)
}
