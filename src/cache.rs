//! Local caching of Remember The Milk entries.

use std::path::Path;

use chrono::Utc;
use sqlx::{migrate::{MigrateDatabase as _, MigrateError}, Sqlite, SqlitePool};

use crate::API;

/// Cache errors
#[derive(thiserror::Error, Debug)]
pub enum CacheError {
    /// Database error
    #[error("Database error")]
    DbOpenError(#[from] sqlx::Error),
    /// Database migration error
    #[error("Database migration error")]
    DbMigrateError(#[from] MigrateError),
    /// Path conversion error
    #[error("File path error")]
    PathError,
    /// Other error
    #[error("Other error")]
    OtherError(#[from] anyhow::Error),
}

/// Task cache result type.
pub type Result<T> = std::result::Result<T, CacheError>;

/// A cache instance
pub struct TaskCache {
    pool: SqlitePool,
    api: API,
}

impl TaskCache {
    /// Open or create a new task cache.
    pub async fn new(db_path: &Path, api: API) -> Result<Self> {
        log::info!("Opening db at {db_path:?}");
        let Some(db_name) = db_path.as_os_str().to_str()
            else {
                return Err(CacheError::PathError);
            };
        if !Sqlite::database_exists(db_name).await? {
            Sqlite::create_database(db_name).await?;                                
        }                                                                       

        let pool = SqlitePool::connect(db_name).await?;                            
        sqlx::migrate!()                                                        
            .run(&pool)                                                         
            .await?;
        Ok(TaskCache { pool, api })

    }

    /// WIP get all tasks
    pub async fn sync(&self) -> Result<()> {
        let last_sync: Option<chrono::DateTime<Utc>> =
            match sqlx::query_as::<_, (chrono::DateTime<Utc>,)>("SELECT last_sync FROM task_meta WHERE id = 1")
                .fetch_one(&self.pool)
                .await
                .map(|(d,)| d) {
                    Ok(d) => Some(d),
                    Err(sqlx::Error::RowNotFound) => None,
                    Err(e) => { return Err(e.into()); }
                };

        eprintln!("last_sync: {last_sync:?}");
        let new_last_sync = Utc::now();
        let tasks = self.api.get_tasks_filtered_sync("", last_sync).await?;
        let count = tasks.list.iter()
            .map(|l| match l.taskseries.as_ref() {
                None => 0,
                Some(l) => l.len(),
            })
            .sum::<usize>();
        eprintln!("Got {count} tasks");

        sqlx::query(
            "INSERT INTO task_meta(id, last_sync)
            VALUES(1, ?1)
            ON CONFLICT(id) DO UPDATE SET
              last_sync=?1
            ")
            .bind(new_last_sync)
            .execute(&self.pool)
            .await?;
        eprintln!("Updated last_sync to {new_last_sync:?}");

        Ok(())
    }
}
