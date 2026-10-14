-- A lease deadline is not a stop timestamp. Existing released permits remain
-- undated rather than inventing completion times for historical records.
ALTER TABLE vrs_permits ADD COLUMN released_at TIMESTAMPTZ;
CREATE INDEX vrs_permit_release_time ON vrs_permits(released_at) WHERE released_at IS NOT NULL;
CREATE INDEX vrs_permit_history ON vrs_permits(created_at DESC, id);
CREATE INDEX vrs_task_created ON vrs_tasks(created_at);
CREATE INDEX vrs_attempt_task_created ON vrs_attempts(task, created_at);
CREATE INDEX vrs_attempt_stop_time ON vrs_attempts(stopped_at) WHERE stopped_at IS NOT NULL;
