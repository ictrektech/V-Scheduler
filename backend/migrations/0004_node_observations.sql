-- Observations never confer worker permissions or change reservation capacity.
CREATE TABLE vrs_observations (
 id TEXT PRIMARY KEY,
 data JSONB NOT NULL DEFAULT '{}',
 observed_at TIMESTAMPTZ,
 last_error TEXT
);
