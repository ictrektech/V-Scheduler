ALTER TABLE vrs_tasks ADD COLUMN retry_at TIMESTAMPTZ;
ALTER TABLE vrs_tasks ADD COLUMN deleted_at TIMESTAMPTZ;
CREATE TABLE vrs_attempts (
 id TEXT PRIMARY KEY REFERENCES vrs_reservations(attempt), task TEXT NOT NULL REFERENCES vrs_tasks(id),
 fence BIGINT NOT NULL, node TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'assigned',
 result JSONB NOT NULL DEFAULT '{}', created_at TIMESTAMPTZ NOT NULL DEFAULT now(), stopped_at TIMESTAMPTZ
);
INSERT INTO vrs_attempts(id,task,fence,node,status,stopped_at)
 SELECT r.attempt,r.task,t.fence,r.node,CASE WHEN r.state='released' THEN t.status ELSE 'assigned' END,r.stopped_at
 FROM vrs_reservations r JOIN vrs_tasks t ON t.id=r.task;
CREATE TABLE vrs_actions (
 task TEXT NOT NULL REFERENCES vrs_tasks(id), key TEXT NOT NULL, action TEXT NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), PRIMARY KEY(task,key)
);
