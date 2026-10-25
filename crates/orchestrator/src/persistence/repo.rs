//! Repositorios de persistencia.

#![forbid(unsafe_code)]

use crate::error::Error;
use crate::persistence::snapshot::{
    SNAPSHOT_SCHEMA_VERSION, SnapshotMeta, StateSnapshot, StoresSnapshot,
};
use async_trait::async_trait;
use serde_json;
use sqlx::Row;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
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
}

/// Crea el directorio y el archivo del estado con los permisos del modelo.
fn preparar_archivo(state_path: &Path) -> Result<(), Error> {
    // Crear directorio padre si no existe. Lo que se crea acá lleva modo 0700:
    // el modelo es el historial de uso de una persona y se guarda en claro.
    // Un directorio que ya existía no se toca: lo eligió quien configuró la ruta.
    if let Some(parent) = state_path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    // La base se crea antes de que SQLite la abra, con modo 0600. SQLite crea
    // el WAL y el SHM con el modo de la base, así que quedan igual. Si ya
    // existía más abierta (una copia, una instalación anterior) se cierra.
    //
    // Por descriptor y sin seguir enlaces: quien pueda escribir en el directorio
    // del estado dejaría un enlace a otro archivo de la persona, y un chmod por
    // ruta se lo cerraría. Y la base tiene que ser de quien corre el servicio.
    let archivo = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state_path)?;
    let duenio = archivo.metadata()?.uid();
    let yo = std::fs::metadata("/proc/self")?.uid();
    if duenio != yo {
        return Err(Error::Other(format!(
            "{} es del uid {duenio} y este servicio corre como {yo}",
            state_path.display()
        )));
    }
    archivo.set_permissions(std::fs::Permissions::from_mode(0o600))?;

    Ok(())
}

impl SqliteRepository {
    /// Crea un nuevo repositorio SQLite.
    pub async fn new(state_path: impl AsRef<Path>) -> Result<Self, Error> {
        let state_path = state_path.as_ref().to_path_buf();

        // En un hilo aparte: son operaciones de archivo bloqueantes.
        let ruta = state_path.clone();
        tokio::task::spawn_blocking(move || preparar_archivo(&ruta))
            .await
            .map_err(|e| {
                Error::Other(format!("la preparación del estado se interrumpió: {e}"))
            })??;

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

        Ok(Self { pool })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn modo(ruta: &Path) -> u32 {
        std::fs::metadata(ruta).unwrap().permissions().mode() & 0o777
    }

    /// El modelo guarda qué ejecutables y bibliotecas usa una persona y en qué
    /// orden: es su historial de uso, y se guarda en claro. Por eso sólo lo lee
    /// su dueña, igual que el de `vasak-health-service`.
    #[tokio::test]
    async fn el_modelo_y_su_directorio_son_solo_de_su_dueno() {
        let dir = tempdir().unwrap();
        let estado = dir.path().join("state/vasak-preload/state.sqlite");

        let repo = SqliteRepository::new(&estado).await.unwrap();
        repo.save(&repo.load().await.unwrap()).await.unwrap();

        assert_eq!(modo(&estado), 0o600, "la base");
        assert_eq!(modo(estado.parent().unwrap()), 0o700, "su directorio");
        // WAL y SHM los crea SQLite con el modo de la base: sin esto el
        // historial se lee desde ellos aunque la base esté cerrada.
        for extra in ["state.sqlite-wal", "state.sqlite-shm"] {
            let ruta = estado.with_file_name(extra);
            // Que existan: si SQLite no los dejara, esto comprobaría nada.
            assert!(ruta.exists(), "{extra} no existe");
            assert_eq!(modo(&ruta), 0o600, "{extra}");
        }
    }

    #[tokio::test]
    async fn una_base_que_ya_estaba_abierta_a_todos_se_cierra() {
        // Una instalación anterior, o una copia, puede dejarla en 0644.
        let dir = tempdir().unwrap();
        let estado = dir.path().join("state.sqlite");
        std::fs::write(&estado, b"").unwrap();
        std::fs::set_permissions(&estado, std::fs::Permissions::from_mode(0o644)).unwrap();

        SqliteRepository::new(&estado).await.unwrap();

        assert_eq!(modo(&estado), 0o600);
    }

    #[tokio::test]
    async fn no_sigue_un_enlace_simbolico_en_la_ruta_del_estado() {
        // Quien pueda escribir en el directorio del estado dejaría un enlace a
        // otro archivo de la persona, y el chmod y SQLite lo seguirían.
        let dir = tempdir().unwrap();
        let ajeno = dir.path().join("ajeno");
        std::fs::write(&ajeno, b"no tocar").unwrap();
        std::fs::set_permissions(&ajeno, std::fs::Permissions::from_mode(0o644)).unwrap();
        let estado = dir.path().join("state.sqlite");
        std::os::unix::fs::symlink(&ajeno, &estado).unwrap();

        assert!(SqliteRepository::new(&estado).await.is_err());

        assert_eq!(modo(&ajeno), 0o644, "el destino del enlace no se toca");
        assert_eq!(std::fs::read(&ajeno).unwrap(), b"no tocar");
    }

    #[tokio::test]
    async fn un_wal_y_un_shm_que_ya_estaban_abiertos_se_cierran() {
        // SQLite sólo copia el modo de la base cuando crea esos archivos: uno que
        // ya existía (una copia del directorio, una versión anterior) se queda
        // como estaba y lee el historial aunque la base esté cerrada.
        let dir = tempdir().unwrap();
        let estado = dir.path().join("state.sqlite");
        for extra in ["state.sqlite-wal", "state.sqlite-shm"] {
            let ruta = dir.path().join(extra);
            std::fs::write(&ruta, b"").unwrap();
            std::fs::set_permissions(&ruta, std::fs::Permissions::from_mode(0o666)).unwrap();
        }

        let repo = SqliteRepository::new(&estado).await.unwrap();
        repo.save(&repo.load().await.unwrap()).await.unwrap();

        for extra in ["state.sqlite-wal", "state.sqlite-shm"] {
            assert_eq!(modo(&dir.path().join(extra)), 0o600, "{extra}");
        }
    }

    #[tokio::test]
    async fn no_se_toca_el_modo_de_un_directorio_que_ya_existia() {
        // El directorio es lo que eligió quien configuró la ruta: sólo se le
        // pone modo a lo que se crea acá.
        let dir = tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        SqliteRepository::new(dir.path().join("state.sqlite"))
            .await
            .unwrap();

        assert_eq!(modo(dir.path()), 0o755);
    }
}
