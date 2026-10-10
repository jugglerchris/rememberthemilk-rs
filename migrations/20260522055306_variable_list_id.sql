-- Remove list_id from the primary key for tasks.

-- Previous migration may have left tasks_old around
DROP TABLE IF EXISTS tasks_old;

ALTER TABLE tasks RENAME TO tasks_old;

CREATE TABLE tasks (
    taskseries_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    deleted BOOLEAN DEFAULT FALSE,
    data JSONB NOT NULL,
    due_time DATETIME GENERATED ALWAYS AS (
        CASE json_extract(data, "$.due")
            WHEN "" THEN NULL
            ELSE CASE json_extract(data, "$.has_due_time")
                WHEN "1" THEN datetime(json_extract(data, "$.due"))
                ELSE datetime(json_extract(data, "$.due"), "+23:59:59")
                END
            END)
        STORED,
    PRIMARY KEY (taskseries_id, task_id)
);
INSERT INTO tasks (taskseries_id, task_id, deleted, data)
   SELECT taskseries_id, task_id, deleted, data
   FROM tasks_old;

-- Now the same for taskseries (but keep list_id)
ALTER TABLE taskseries RENAME TO taskseries_old;
CREATE TABLE taskseries (
    list_id TEXT NOT NULL,
    taskseries_id TEXT NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (taskseries_id)
);

INSERT INTO taskseries (list_id, taskseries_id, data)
   SELECT list_id, taskseries_id, data
   FROM taskseries_old;

-- Delete temporary tables
DROP TABLE tasks_old;
DROP TABLE taskseries_old
