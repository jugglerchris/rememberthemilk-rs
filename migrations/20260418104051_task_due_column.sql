-- Add a due date and then set it.
ALTER TABLE tasks RENAME TO tasks_old;

CREATE TABLE tasks (
    list_id TEXT NOT NULL,
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
    PRIMARY KEY (list_id, taskseries_id, task_id)
);
INSERT INTO tasks (list_id, taskseries_id, task_id, deleted, data)
   SELECT list_id, taskseries_id, task_id, deleted, data
   FROM tasks_old;
