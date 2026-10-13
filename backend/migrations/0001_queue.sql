CREATE TABLE vrs_nodes (
 id TEXT PRIMARY KEY, capacities JSONB NOT NULL, report JSONB NOT NULL DEFAULT '{}',
 heartbeat TIMESTAMPTZ, session TEXT, draining BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE vrs_tasks (
 id TEXT PRIMARY KEY, app TEXT NOT NULL, idem TEXT NOT NULL, hash TEXT NOT NULL,
 body JSONB NOT NULL, status TEXT NOT NULL DEFAULT 'queued', version BIGINT NOT NULL DEFAULT 1,
 attempt TEXT, fence BIGINT NOT NULL DEFAULT 0, node TEXT REFERENCES vrs_nodes(id),
 progress JSONB NOT NULL DEFAULT '{}', result JSONB NOT NULL DEFAULT '{}',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(app, idem)
);
CREATE TABLE vrs_reservations (
 attempt TEXT PRIMARY KEY, task TEXT NOT NULL REFERENCES vrs_tasks(id), node TEXT NOT NULL REFERENCES vrs_nodes(id),
 resources JSONB NOT NULL, state TEXT NOT NULL DEFAULT 'held',
 lease_until TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 seconds',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), stopped_at TIMESTAMPTZ
);
CREATE TABLE vrs_events (
 cursor BIGSERIAL PRIMARY KEY, task TEXT NOT NULL REFERENCES vrs_tasks(id), app TEXT NOT NULL,
 status TEXT NOT NULL, version BIGINT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX vrs_queue ON vrs_tasks(status, created_at);
CREATE INDEX vrs_reserved_node ON vrs_reservations(node, state);
CREATE INDEX vrs_event_app ON vrs_events(app, cursor);
