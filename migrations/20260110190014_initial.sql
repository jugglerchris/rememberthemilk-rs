CREATE TABLE taskseries (
    list_id TEXT NOT NULL,
    taskseries_id TEXT NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (list_id, taskseries_id)
);

CREATE TABLE tasks (
    list_id TEXT NOT NULL,
    taskseries_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    deleted BOOLEAN DEFAULT FALSE,
    data JSONB NOT NULL,
    PRIMARY KEY (list_id, taskseries_id, task_id)
);

CREATE TABLE task_meta (
    id INTEGER PRIMARY KEY,
    last_sync DATETIME
);
