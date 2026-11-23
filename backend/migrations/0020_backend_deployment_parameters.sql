ALTER TABLE vrs_backend_deployments
    ADD COLUMN requested_host_port integer CHECK (requested_host_port BETWEEN 42000 AND 42999),
    ADD COLUMN cpu_millicores integer CHECK (cpu_millicores BETWEEN 100 AND 128000),
    ADD COLUMN memory_mib integer CHECK (memory_mib BETWEEN 16 AND 1048576),
    ADD COLUMN config jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(config) = 'object');
