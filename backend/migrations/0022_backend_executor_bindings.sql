CREATE TABLE vrs_backend_executor_bindings (
    executor_id TEXT PRIMARY KEY REFERENCES vrs_executor_settings(id),
    deployment_id TEXT NOT NULL REFERENCES vrs_backend_deployments(id),
    transfer_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    profile TEXT NOT NULL,
    model_limits JSONB NOT NULL,
    model_parameters JSONB NOT NULL,
    UNIQUE(deployment_id, model_id)
);
