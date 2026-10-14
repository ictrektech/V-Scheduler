ALTER TABLE vrs_permits ADD COLUMN parent_attempt TEXT REFERENCES vrs_reservations(attempt);
ALTER TABLE vrs_permits ADD COLUMN delegated_resources JSONB NOT NULL DEFAULT '{}';
CREATE INDEX vrs_permits_parent ON vrs_permits(parent_attempt) WHERE parent_attempt IS NOT NULL;
