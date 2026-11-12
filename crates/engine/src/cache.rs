use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;
use redb::{Database, TableDefinition};

use crate::error::EngineError;
use crate::models::{CommitChurnStats, LanguageBlobStats};

/// Length in bytes of a standard SHA-1 Git object identifier.
pub const OID_BYTE_LEN: usize = 20;

const CACHE_FILE_NAME: &str = "oid_cache_v1.redb";
const COMMIT_CHURN_TABLE: TableDefinition<'_, &[u8; OID_BYTE_LEN], &[u8]> = TableDefinition::new("commit_churn_v1");
const BLOB_LANG_TABLE: TableDefinition<'_, &[u8; OID_BYTE_LEN], &[u8]> = TableDefinition::new("blob_lang_v1");

/// Thread-safe persistent `OID Cache` backed by an embedded `redb` database.
#[derive(Clone)]
pub struct OidCache {
    db: Arc<Database>,
    path: PathBuf,
}

impl OidCache {
    /// Opens or creates the default user-scoped `OID Cache` inside the XDG cache directory.
    pub fn open_default() -> Result<Self, EngineError> {
        let cache_dir: PathBuf = if let Some(proj_dirs) = ProjectDirs::from("dev", "git-oxide", "git-oxide") {
            proj_dirs.cache_dir().to_path_buf()
        } else {
            std::env::temp_dir().join("git-oxide-cache")
        };

        fs::create_dir_all(&cache_dir).map_err(|error| {
            EngineError::CacheError(format!("failed to create cache directory '{}': {error}", cache_dir.display()))
        })?;

        let db_path: PathBuf = cache_dir.join(CACHE_FILE_NAME);
        Self::open_at(&db_path)
    }

    /// Opens or creates an `OID Cache` database at the specified file path.
    pub fn open_at(path: &Path) -> Result<Self, EngineError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                EngineError::CacheError(format!("failed to create parent directory '{}': {error}", parent.display()))
            })?;
        }

        let db: Database = Database::create(path).map_err(|error| {
            EngineError::CacheError(format!("failed to open redb database at '{}': {error}", path.display()))
        })?;

        let write_txn = db
            .begin_write()
            .map_err(|error| EngineError::CacheError(format!("failed to begin initial write transaction: {error}")))?;
        {
            let _ = write_txn.open_table(COMMIT_CHURN_TABLE).map_err(|error| {
                EngineError::CacheError(format!("failed to initialize commit churn table: {error}"))
            })?;
            let _ = write_txn.open_table(BLOB_LANG_TABLE).map_err(|error| {
                EngineError::CacheError(format!("failed to initialize blob language table: {error}"))
            })?;
        }
        write_txn
            .commit()
            .map_err(|error| EngineError::CacheError(format!("failed to commit table initialization: {error}")))?;

        Ok(Self { db: Arc::new(db), path: path.to_path_buf() })
    }

    /// Returns the filesystem path of the underlying `redb` database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Streams cached `CommitChurnStats` entries for `oids` in one read transaction without
    /// retaining all decoded `touched_paths` vectors in memory simultaneously.
    pub fn scan_commit_churn_batch<F>(&self, oids: &[[u8; OID_BYTE_LEN]], mut on_hit: F) -> Result<usize, EngineError>
    where
        F: FnMut(usize, CommitChurnStats),
    {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|error| EngineError::CacheError(format!("failed to begin read transaction: {error}")))?;
        let table = read_txn
            .open_table(COMMIT_CHURN_TABLE)
            .map_err(|error| EngineError::CacheError(format!("failed to open commit churn table: {error}")))?;

        let config = bincode::config::standard();
        let mut hits: usize = 0;

        for (idx, oid) in oids.iter().enumerate() {
            if let Some(guard) = table
                .get(oid)
                .map_err(|error| EngineError::CacheError(format!("failed to read commit churn entry: {error}")))?
            {
                let bytes: &[u8] = guard.value();
                let (decoded, _): (CommitChurnStats, usize) = bincode::serde::decode_from_slice(bytes, config)
                    .map_err(|error| {
                        EngineError::SerializationError(format!("failed to decode commit churn entry: {error}"))
                    })?;
                on_hit(idx, decoded);
                hits += 1;
            }
        }

        Ok(hits)
    }

    /// Looks up cached `CommitChurnStats` for a batch of 20-byte commit OIDs in one read transaction.
    pub fn get_commit_churn_batch(
        &self,
        oids: &[[u8; OID_BYTE_LEN]],
    ) -> Result<HashMap<[u8; OID_BYTE_LEN], CommitChurnStats>, EngineError> {
        let mut results: HashMap<[u8; OID_BYTE_LEN], CommitChurnStats> = HashMap::new();
        self.scan_commit_churn_batch(oids, |idx, decoded| {
            results.insert(oids[idx], decoded);
        })?;
        Ok(results)
    }

    /// Writes a batch of `(commit_oid, CommitChurnStats)` entries in a single write transaction.
    pub fn insert_commit_churn_batch(
        &self,
        entries: &[([u8; OID_BYTE_LEN], CommitChurnStats)],
    ) -> Result<(), EngineError> {
        if entries.is_empty() {
            return Ok(());
        }

        let config = bincode::config::standard();
        let mut encoded_entries: Vec<([u8; OID_BYTE_LEN], Vec<u8>)> = Vec::with_capacity(entries.len());
        for (oid, stats) in entries {
            let payload: Vec<u8> = bincode::serde::encode_to_vec(stats, config).map_err(|error| {
                EngineError::SerializationError(format!("failed to encode commit churn entry: {error}"))
            })?;
            encoded_entries.push((*oid, payload));
        }

        let write_txn = self
            .db
            .begin_write()
            .map_err(|error| EngineError::CacheError(format!("failed to begin write transaction: {error}")))?;
        {
            let mut table = write_txn
                .open_table(COMMIT_CHURN_TABLE)
                .map_err(|error| EngineError::CacheError(format!("failed to open commit churn table: {error}")))?;
            for (oid, payload) in &encoded_entries {
                table.insert(oid, payload.as_slice()).map_err(|error| {
                    EngineError::CacheError(format!("failed to insert commit churn entry: {error}"))
                })?;
            }
        }
        write_txn
            .commit()
            .map_err(|error| EngineError::CacheError(format!("failed to commit churn batch transaction: {error}")))?;

        Ok(())
    }

    /// Looks up cached `LanguageBlobStats` for a batch of 20-byte blob OIDs in one read transaction.
    pub fn get_blob_lang_batch(
        &self,
        oids: &[[u8; OID_BYTE_LEN]],
    ) -> Result<HashMap<[u8; OID_BYTE_LEN], LanguageBlobStats>, EngineError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|error| EngineError::CacheError(format!("failed to begin read transaction: {error}")))?;
        let table = read_txn
            .open_table(BLOB_LANG_TABLE)
            .map_err(|error| EngineError::CacheError(format!("failed to open blob language table: {error}")))?;

        let mut results: HashMap<[u8; OID_BYTE_LEN], LanguageBlobStats> = HashMap::with_capacity(oids.len());
        let config = bincode::config::standard();

        for oid in oids {
            if let Some(guard) = table
                .get(oid)
                .map_err(|error| EngineError::CacheError(format!("failed to read blob language entry: {error}")))?
            {
                let bytes: &[u8] = guard.value();
                let (decoded, _): (LanguageBlobStats, usize) = bincode::serde::decode_from_slice(bytes, config)
                    .map_err(|error| {
                        EngineError::SerializationError(format!("failed to decode blob language entry: {error}"))
                    })?;
                results.insert(*oid, decoded);
            }
        }

        Ok(results)
    }

    /// Writes a batch of `(blob_oid, LanguageBlobStats)` entries in a single write transaction.
    pub fn insert_blob_lang_batch(
        &self,
        entries: &[([u8; OID_BYTE_LEN], LanguageBlobStats)],
    ) -> Result<(), EngineError> {
        if entries.is_empty() {
            return Ok(());
        }

        let config = bincode::config::standard();
        let mut encoded_entries: Vec<([u8; OID_BYTE_LEN], Vec<u8>)> = Vec::with_capacity(entries.len());
        for (oid, stats) in entries {
            let payload: Vec<u8> = bincode::serde::encode_to_vec(stats, config).map_err(|error| {
                EngineError::SerializationError(format!("failed to encode blob language entry: {error}"))
            })?;
            encoded_entries.push((*oid, payload));
        }

        let write_txn = self
            .db
            .begin_write()
            .map_err(|error| EngineError::CacheError(format!("failed to begin write transaction: {error}")))?;
        {
            let mut table = write_txn
                .open_table(BLOB_LANG_TABLE)
                .map_err(|error| EngineError::CacheError(format!("failed to open blob language table: {error}")))?;
            for (oid, payload) in &encoded_entries {
                table.insert(oid, payload.as_slice()).map_err(|error| {
                    EngineError::CacheError(format!("failed to insert blob language entry: {error}"))
                })?;
            }
        }
        write_txn.commit().map_err(|error| {
            EngineError::CacheError(format!("failed to commit blob language batch transaction: {error}"))
        })?;

        Ok(())
    }
}
