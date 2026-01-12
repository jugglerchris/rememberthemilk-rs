CREATE TABLE taskseries (
    id INTEGER PRIMARY KEY,
    list_id TEXT NOT NULL,
    taskseries_id TEXT NOT NULL,
    data JSONB NOT NULL
);

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    list_id TEXT NOT NULL,
    taskseries_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    data JSONB NOT NULL
);

CREATE TABLE task_meta (
    id INTEGER PRIMARY KEY,
    last_sync DATETIME
);
