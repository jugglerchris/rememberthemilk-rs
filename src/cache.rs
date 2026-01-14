//! Local caching of Remember The Milk entries.

use std::{path::Path, time::Instant};

use chrono::Utc;
use sqlx::{migrate::{MigrateDatabase as _, MigrateError}, Sqlite, SqlitePool};
type JsonValue = serde_json::Value;

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
    /// Error parsing response
    #[error("Error parsing RTM response")]
    ParseError(&'static str),
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

        log::info!("last_sync: {last_sync:?}");
        let new_last_sync = Utc::now();
        let mut tasks = self.api.get_tasks_filtered_sync_json("", last_sync).await?;

        let now = Instant::now();
        let mut tx = self.pool.begin().await?;
        let lists = tasks.get_mut("list");
        if let Some(JsonValue::Array(values)) = lists {
            for list in values {
                let list_id = list.get("id").unwrap().as_str().unwrap().to_string();
                if let Some(JsonValue::Array(taskseries)) = list.get_mut("taskseries") {
                    for ts in taskseries {
                        let taskseries_id = ts.get("id").unwrap().as_str().unwrap().to_string();
                        // Extract the task to put it into the separate table.
                        let task = ts.get_mut("task").map(|t| t.take());
                        sqlx::query(
                            "INSERT INTO taskseries(list_id, taskseries_id, data)
                            VALUES(?, ?, ?)
                        ")
                            .bind(&list_id)
                            .bind(&taskseries_id)
                            .bind(ts.to_string())
                            .execute(&mut *tx)
                            .await?;

                        if let Some(JsonValue::Array(tasks)) = task {
                            for t in tasks {
                                let task_id = t.get("id").unwrap().as_str().unwrap();
                                sqlx::query(
                                    "INSERT INTO tasks(list_id, taskseries_id, task_id, data)
                                    VALUES(?, ?, ?, ?)
                                ")
                                    .bind(&list_id)
                                    .bind(&taskseries_id)
                                    .bind(task_id)
                                    .bind(ts.to_string())
                                    .execute(&mut *tx)
                                    .await?;
                            }
                        }
                    }
                }
            }
        }
        tx.commit().await?;
        log::info!("Inserting data took: {} seconds", now.elapsed().as_secs());

        sqlx::query(
            "INSERT INTO task_meta(id, last_sync)
            VALUES(1, ?1)
            ON CONFLICT(id) DO UPDATE SET
              last_sync=?1
            ")
            .bind(new_last_sync)
            .execute(&self.pool)
            .await?;
        log::info!("Updated last_sync to {new_last_sync:?}");

        Ok(())
    }
}
