CREATE TABLE vrs_backend_deployment_access (
    app_id text PRIMARY KEY,
    targets jsonb NOT NULL,
    repositories jsonb NOT NULL
);

CREATE TABLE vrs_backend_deployments (
    id text PRIMARY KEY,
    app_id text NOT NULL,
    request_key text NOT NULL,
    target_id text NOT NULL,
    target_kind text NOT NULL CHECK (target_kind IN ('ssh', 'cluster')),
    profile text NOT NULL,
    host text NOT NULL,
    host_port integer NOT NULL CHECK (host_port BETWEEN 42000 AND 42999),
    container_port integer NOT NULL CHECK (container_port BETWEEN 1 AND 65535),
    image text NOT NULL,
    nvidia boolean NOT NULL,
    state text NOT NULL CHECK (state IN ('pending', 'running', 'failed', 'stopping', 'stopped', 'deleting', 'removed', 'cleanup_failed')),
    last_error text,
    retry_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (app_id, request_key)
);

CREATE UNIQUE INDEX vrs_backend_deployment_host_port
    ON vrs_backend_deployments (host, host_port) WHERE state <> 'removed';
CREATE INDEX vrs_backend_deployment_jobs
    ON vrs_backend_deployments (retry_at) WHERE state IN ('pending', 'running', 'stopping', 'deleting');
