/// SQL Query for retrieving all current tasks tasks
pub(crate) const GET_TASKS: &str = "SELECT * FROM tasks OFFSET $1 LIMIT $2;";

/// SQL Query for retrieving all current tasks by group ID
pub(crate) const GET_TASKS_BY_GROUP_ID: &str =
    "SELECT * FROM tasks WHERE group_id = $1 OFFSET $2 LIMIT $3;";

/// SQL Query for retrieving a single task
pub(crate) const GET_TASK_BY_ID: &str = "SELECT * FROM tasks WHERE id = $1;";

/// SQL Query for leasing a single task for work
pub(crate) const LEASE_TASK: &str = r#"
WITH existing AS (
  SELECT
    id
  FROM tasks
  WHERE id = $1
    AND status = 'pending'
    AND lease_worker_id IS NULL
  FOR UPDATE SKIP LOCKED
)
UPDATE tasks
SET status = 'leased'
  , leased_at = NOW()
  , lease_worker_id = $2
  , last_updated_at = NOW()
WHERE
  (SELECT id FROM existing) IS NOT NULL AND id = (SELECT id FROM existing)
RETURNING *
"#;

/// SQL Query for releasing a single task (i.e. so another worker can lease it)
pub(crate) const RELEASE_TASK: &str = r#"
WITH existing AS  (
  SELECT
    id
  FROM tasks
  WHERE id = $1
    AND lease_id = $2
    AND lease_worker_id = $3
    AND status = 'leased'
)
UPDATE tasks
SET status = 'pending'
  , lease_id = uuid_generate_v1mc()
  , lease_worker_id = NULL
  , leased_at = NULL
  , last_updated_at = NOW()
WHERE
  (SELECT id FROM existing) IS NOT NULL AND id = (SELECT id FROM existing)
RETURNING *
"#;

/// SQL Query for updating a single task
pub(crate) const INSERT_TASK: &str =
    "INSERT INTO tasks (group_id, data_json) VALUES ($1, $2::jsonb) RETURNING *;";

/// SQL Query for marking a single task as complete
pub(crate) const MARK_TASK_COMPLETE: &str = r#"
WITH existing AS (
  SELECT
    id
  FROM tasks
  WHERE id = $1
    AND lease_worker_id = $2
    AND status != 'completed'
)
UPDATE tasks
SET status = 'completed'
  , completed_at = NOW()
  , last_updated_at = NOW()
WHERE
  (SELECT id FROM existing) IS NOT NULL AND id = (SELECT id FROM existing)
RETURNING *
"#;

/// SQL Query for marking a single task as complete
pub(crate) const MARK_TASK_FAILED: &str = r#"
WITH existing AS (
SELECT id
FROM tasks
WHERE id = $1
  AND lease_worker_id = $2
)
UPDATE tasks
SET status = 'failed'
  , last_failure_reason = $3
  , last_failed_at = NOW()
  , last_updated_at = NOW()
WHERE
  (SELECT id FROM existing) IS NOT NULL AND id = (SELECT id FROM existing)
RETURNING *
"#;
