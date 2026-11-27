CREATE TABLE vrs_backend_model_deletions (
    deployment_id TEXT NOT NULL REFERENCES vrs_backend_deployments(id),
    model_id TEXT NOT NULL,
    PRIMARY KEY (deployment_id, model_id)
);
