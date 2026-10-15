-- Enrollment is authorization to request access, never a resource grant.
ALTER TABLE vrs_nodes ADD COLUMN management_enabled BOOLEAN NOT NULL DEFAULT TRUE;
CREATE TABLE vrs_enrollments (
 id TEXT PRIMARY KEY, node_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
 discovery_source TEXT NOT NULL DEFAULT '', discovery_id TEXT NOT NULL DEFAULT '',
 expires_at TIMESTAMPTZ NOT NULL, used_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_worker_registrations (
 node_id TEXT PRIMARY KEY, enrollment_id TEXT NOT NULL REFERENCES vrs_enrollments(id),
 display_name TEXT NOT NULL, instance_id TEXT NOT NULL, boot_id TEXT NOT NULL,
 session_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
 state TEXT NOT NULL CHECK(state IN ('pending','approved','revoked')),
 version BIGINT NOT NULL DEFAULT 1,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_node_settings (
 node_id TEXT PRIMARY KEY REFERENCES vrs_nodes(id), display_name TEXT NOT NULL,
 definition JSONB NOT NULL, version BIGINT NOT NULL DEFAULT 1,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_executor_settings (
 id TEXT PRIMARY KEY, node_id TEXT NOT NULL REFERENCES vrs_nodes(id),
 definition JSONB NOT NULL, state TEXT NOT NULL CHECK(state IN ('pending','enabled','disabled')),
 version BIGINT NOT NULL DEFAULT 1,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_service_identities (
 id TEXT PRIMARY KEY, app_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
 delegated_executors JSONB NOT NULL DEFAULT '[]', permit_executors JSONB NOT NULL DEFAULT '[]',
 revoked_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_management_events (
 cursor BIGSERIAL PRIMARY KEY, actor TEXT NOT NULL, action TEXT NOT NULL,
 resource TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX vrs_management_pending ON vrs_worker_registrations(state,created_at);

CREATE UNIQUE INDEX vrs_node_discovery_identity ON vrs_node_settings((definition->>'discovery_source'),(definition->>'discovery_id')) WHERE definition->>'discovery_source'<>'';
-- Parent vrs_permits retains the head-node allocation for backward compatibility.
CREATE TABLE vrs_permit_placements (
 permit_id TEXT NOT NULL REFERENCES vrs_permits(id), node_id TEXT NOT NULL REFERENCES vrs_nodes(id),
 resources JSONB NOT NULL, PRIMARY KEY(permit_id,node_id)
);
CREATE INDEX vrs_permit_placement_node ON vrs_permit_placements(node_id);

-- Source planning is deliberately separate from executable worker nodes.
CREATE TABLE vrs_source_node_settings (
 source TEXT NOT NULL CHECK(source='modelinfer'), source_id TEXT NOT NULL,
 display_name TEXT NOT NULL, definition JSONB NOT NULL,
 version BIGINT NOT NULL DEFAULT 1, updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(source,source_id)
);
