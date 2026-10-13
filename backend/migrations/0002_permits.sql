CREATE TABLE vrs_permits (
 id TEXT PRIMARY KEY, app TEXT NOT NULL, idem TEXT NOT NULL, executor TEXT NOT NULL,
 node TEXT NOT NULL REFERENCES vrs_nodes(id), resources JSONB NOT NULL,
 state TEXT NOT NULL DEFAULT 'active', evidence TEXT,
 lease_until TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 seconds',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE(app,idem)
);
CREATE INDEX vrs_permit_node ON vrs_permits(node,state);
