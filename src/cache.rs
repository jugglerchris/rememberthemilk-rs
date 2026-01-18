//! Local caching of Remember The Milk entries.

use std::{path::Path, time::Instant};

use chrono::Utc;
use sqlx::{
    migrate::{MigrateDatabase as _, MigrateError},
    Sqlite, SqlitePool,
};
type JsonValue = serde_json::Value;

use crate::{RTMList, RTMLists, RTMTasks, RTMTimeline, RTMTransaction, Task, TaskSeries, API};

mod filter;

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
#[derive(Clone)]
pub struct TaskCache {
    pool: SqlitePool,
    api: API,
}

impl TaskCache {
    /// Open or create a new task cache.
    pub async fn new(db_path: &Path, api: API) -> Result<Self> {
        log::info!("Opening db at {db_path:?}");
        let Some(db_name) = db_path.as_os_str().to_str() else {
            return Err(CacheError::PathError);
        };
        if !Sqlite::database_exists(db_name).await? {
            Sqlite::create_database(db_name).await?;
        }

        let pool = SqlitePool::connect(db_name).await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(TaskCache { pool, api })
    }

    /// WIP get all tasks
    pub async fn sync(&self) -> Result<()> {
        let last_sync: Option<chrono::DateTime<Utc>> =
            match sqlx::query_as::<_, (chrono::DateTime<Utc>,)>(
                "SELECT last_sync FROM task_meta WHERE id = 1",
            )
            .fetch_one(&self.pool)
            .await
            .map(|(d,)| d)
            {
                Ok(d) => Some(d),
                Err(sqlx::Error::RowNotFound) => None,
                Err(e) => {
                    return Err(e.into());
                }
            };

        log::info!("last_sync: {last_sync:?}");
        let new_last_sync = Utc::now();
        let mut tasks = self.api.get_tasks_filtered_sync_json("", last_sync).await?;
        //dbg!(&tasks);

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
                            VALUES(?1, ?2, jsonb(?3))
                            ON CONFLICT DO UPDATE SET data = jsonb(?3);
                        ",
                        )
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
                                    VALUES(?1, ?2, ?3, jsonb(?4))
                                    ON CONFLICT DO UPDATE SET data = jsonb(?4);
                                ",
                                )
                                .bind(&list_id)
                                .bind(&taskseries_id)
                                .bind(task_id)
                                .bind(t.to_string())
                                .execute(&mut *tx)
                                .await?;
                            }
                        }
                    }
                }
                if let Some(JsonValue::Array(deleted)) = list.get("deleted") {
                    log::info!("List has {} deleted entries", deleted.len());
                    for entry in deleted {
                        log::debug!("Deleted entry: {entry}");
                        if let Some(ts) = entry.get("taskseries") {
                            log::debug!("Deleted ts: {ts}");
                            let taskseries_id = ts.get("id").unwrap().as_str().unwrap().to_string();
                            if let Some(JsonValue::Array(tasks)) = ts.get("task") {
                                for t in tasks {
                                    log::debug!("Deleted task: {t}");
                                    let task_id = t.get("id").unwrap().as_str().unwrap();
                                    log::info!("Deleting task {list_id}/{taskseries_id}/{task_id}");
                                    sqlx::query(
                                        "UPDATE tasks
                                         SET deleted=true
                                         WHERE list_id=? AND taskseries_id=? AND task_id=?;
                            ",
                                    )
                                    .bind(&list_id)
                                    .bind(&taskseries_id)
                                    .bind(task_id)
                                    .execute(&mut *tx)
                                    .await?;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Now get the lists
        let lists = self.api.get_lists().await?;
        for list in lists {
            sqlx::query(
                    "INSERT INTO lists(list_id, name)
                    VALUES(?1, ?2)
                    ON CONFLICT DO UPDATE SET name = ?2;
                    ",
                    )
                .bind(&list.id)
                .bind(&list.name)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        log::info!("Inserting data took: {} seconds", now.elapsed().as_secs());

        sqlx::query(
            "INSERT INTO task_meta(id, last_sync)
            VALUES(1, ?1)
            ON CONFLICT(id) DO UPDATE SET
              last_sync=?1
            ",
        )
        .bind(new_last_sync)
        .execute(&self.pool)
        .await?;
        log::info!("Updated last_sync to {new_last_sync:?}");

        Ok(())
    }

    /// Return tasks from the cache matching the filter.
    pub async fn get_tasks_filtered(
        &self,
        filt: &str,
    ) -> std::result::Result<RTMTasks, crate::Error> {
        let mut filter_clause = String::new();
        let mut filter_binds = Vec::new();
        if !filt.is_empty() {
            let filter = filter::parse_filter(filt)?;
            let mut context = filter::FilterContext::default();
            let lists = self.get_lists().await?;
            for list in lists {
                context.lists_name_to_id.insert(list.name, list.id);
            }
            let (clause, binds) = filter.to_sqlite_where_clause(&context)?;
            filter_clause = clause;
            filter_binds = binds;

            log::info!("Filter clause: {filter_clause}");
        }

        #[derive(sqlx::FromRow)]
        struct Data {
            list_id: String,
            ts_data: String,
            t_data: String,
        }

        let query_str = format!(
            r#"SELECT ts.list_id, json(ts.data) as ts_data, json(t.data) as t_data
             FROM taskseries ts, tasks t
             USING (list_id, taskseries_id)
             WHERE
                t.deleted != TRUE AND
                {filter_clause};
                "#
        );
        let mut query = sqlx::query_as(&query_str);
        for bind in filter_binds {
            query = query.bind(bind);
        }
        let data: Vec<Data> = query.fetch_all(&self.pool).await?;
        let mut result = RTMTasks {
            rev: Default::default(),
            list: Vec::new(),
        };
        for item in data {
            let mut list = RTMLists {
                id: item.list_id,
                taskseries: None,
            };
            let mut ts_json: serde_json::Value = serde_json::from_str(&item.ts_data).unwrap();
            let t_json: serde_json::Value =
                vec![serde_json::from_str::<serde_json::Value>(&item.t_data).unwrap()].into();
            ts_json
                .as_object_mut()
                .unwrap()
                .insert("task".to_string(), t_json);
            let ts: TaskSeries = serde_json::from_value(ts_json).unwrap();
            list.taskseries = Some(vec![ts]);
            result.list.push(list);
        }
        Ok(result)
    }

    /// Return tasks which are children of a given task
    pub async fn get_task_children(
        &self,
        parent_id: &str,
    ) -> std::result::Result<RTMTasks, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct Data {
            list_id: String,
            ts_data: String,
            t_data: String,
        }

        let query = 
            r#"SELECT ts.list_id, json(ts.data) as ts_data, json(t.data) as t_data
             FROM taskseries ts, tasks t
             USING (list_id, taskseries_id)
             WHERE
                t.deleted != TRUE AND
                jsonb_extract(t.data, "$.completed") = "" AND
                jsonb_extract(ts.data, "$.parent_task_id") = ?
                "#;
        let data: Vec<Data> = sqlx::query_as(query)
            .bind(parent_id)
            .fetch_all(&self.pool).await?;
        let mut result = RTMTasks {
            rev: Default::default(),
            list: Vec::new(),
        };
        for item in data {
            let mut list = RTMLists {
                id: item.list_id,
                taskseries: None,
            };
            let mut ts_json: serde_json::Value = serde_json::from_str(&item.ts_data).unwrap();
            let t_json: serde_json::Value =
                vec![serde_json::from_str::<serde_json::Value>(&item.t_data).unwrap()].into();
            ts_json
                .as_object_mut()
                .unwrap()
                .insert("task".to_string(), t_json);
            let ts: TaskSeries = serde_json::from_value(ts_json).unwrap();
            list.taskseries = Some(vec![ts]);
            result.list.push(list);
        }
        Ok(result)
    }

    /// Add a task and update the cache.
    pub async fn add_task(
        &self,
        timeline: &RTMTimeline,
        name: &str,
        list: Option<&RTMLists>,
        parent: Option<&Task>,
        external_id: Option<&str>,
        smart: bool,
    ) -> std::result::Result<Option<RTMLists>, crate::Error> {
        let result = self
            .api
            .add_task(timeline, name, list, parent, external_id, smart)
            .await?;
        self.sync().await?;
        Ok(result)
    }

    /// Get a new timeline
    pub async fn get_timeline(&self) -> std::result::Result<RTMTimeline, crate::Error> {
        self.api.get_timeline().await
    }
    /// Get lists
    pub async fn get_lists(&self) -> std::result::Result<Vec<RTMList>, crate::Error> {
        self.api.get_lists().await
    }
    /// Mark complete
    pub async fn mark_complete(
        &self,
        timeline: &RTMTimeline,
        list: &RTMLists,
        taskseries: &TaskSeries,
        task: &Task,
    ) -> std::result::Result<Option<RTMTransaction>, crate::Error> {
        let result = self
            .api
            .mark_complete(timeline, list, taskseries, task)
            .await?;
        self.sync().await?;
        Ok(result)
    }
    /// Undo transaction
    pub async fn undo_transaction(
        &self,
        timeline: &RTMTimeline,
        transaction_id: &str,
    ) -> std::result::Result<(), crate::Error> {
        self.api.undo_transaction(timeline, transaction_id).await
    }
}
