//! Repositorios de persistencia.

#![forbid(unsafe_code)]

use crate::error::Error;
use crate::persistence::snapshot::{
    ExeMapRecord, ExeRecord, MapRecord, MarkovRecord, SNAPSHOT_SCHEMA_VERSION, SnapshotMeta,
    StateSnapshot, StoresSnapshot,
};
use async_trait::async_trait;
use serde_json;
use sqlx::Row;
use std::path::Path;
use std::time::SystemTime;

/// Repositorio de estado.
#[async_trait]
pub trait StateRepository: Send + Sync {
    /// Carga el estado desde el repositorio.
    async fn load(&self) -> Result<StoresSnapshot, Error>;

    /// Guarda el estado en el repositorio.
    async fn save(&self, snapshot: &StoresSnapshot) -> Result<(), Error>;
}

/// Repositorio que no hace nada (para testing).
#[derive(Debug, Default)]
pub struct NoopRepository;

#[async_trait]
impl StateRepository for NoopRepository {
    async fn load(&self) -> Result<StoresSnapshot, Error> {
        Ok(StoresSnapshot {
            meta: SnapshotMeta {
                schema_version: SNAPSHOT_SCHEMA_VERSION,
                app_version: None,
                created_at: Some(
                    SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                ),
            },
            state: StateSnapshot {
                model_time: 0,
                last_accounting_time: 0,
                exes: Vec::new(),
                maps: Vec::new(),
                exe_maps: Vec::new(),
                markov_edges: Vec::new(),
            },
        })
    }

    async fn save(&self, _snapshot: &StoresSnapshot) -> Result<(), Error> {
        Ok(())
    }
}

/// Repositorio SQLite usando serde_json para serialización.
#[derive(Debug)]
pub struct SqliteRepository {
    pool: sqlx::SqlitePool,
    state_path: std::path::PathBuf,
}

impl SqliteRepository {
    /// Crea un nuevo repositorio SQLite.
    pub async fn new(state_path: impl AsRef<Path>) -> Result<Self, Error> {
        let state_path = state_path.as_ref().to_path_buf();

        // Crear directorio padre si no existe
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&state_path)
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::from_secs(30)),
            )
            .await?;

        // Crear tabla si no existe
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS snapshots (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data BLOB NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool, state_path })
    }
}

#[async_trait]
impl StateRepository for SqliteRepository {
    async fn load(&self) -> Result<StoresSnapshot, Error> {
        let row = sqlx::query("SELECT data FROM snapshots WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(StoresSnapshot {
                meta: SnapshotMeta {
                    schema_version: SNAPSHOT_SCHEMA_VERSION,
                    app_version: None,
                    created_at: Some(
                        SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    ),
                },
                state: StateSnapshot {
                    model_time: 0,
                    last_accounting_time: 0,
                    exes: Vec::new(),
                    maps: Vec::new(),
                    exe_maps: Vec::new(),
                    markov_edges: Vec::new(),
                },
            });
        };

        let data: Vec<u8> = row.get("data");
        let snapshot: StoresSnapshot = serde_json::from_slice(&data)
            .map_err(|e| Error::Other(format!("deserialización fallida: {}", e)))?;

        // Verificar versión de esquema
        if snapshot.meta.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(Error::Other(format!(
                "versión de esquema incompatible: esperado {}, encontrado {}",
                SNAPSHOT_SCHEMA_VERSION, snapshot.meta.schema_version
            )));
        }

        Ok(snapshot)
    }

    async fn save(&self, snapshot: &StoresSnapshot) -> Result<(), Error> {
        let data = serde_json::to_vec(snapshot)
            .map_err(|e| Error::Other(format!("serialización fallida: {}", e)))?;

        let updated_at = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        sqlx::query(
            r#"
            INSERT INTO snapshots (id, data, updated_at)
            VALUES (1, ?, ?)
            ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at
            "#,
        )
        .bind(data)
        .bind(updated_at as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}