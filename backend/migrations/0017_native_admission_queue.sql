-- Admission hints contain identity and ordering only, never inference payloads.
ALTER TABLE vrs_inference_waiters ADD COLUMN force_admission BOOLEAN NOT NULL DEFAULT false;
CREATE OR REPLACE FUNCTION vrs_priority(options JSONB) RETURNS INTEGER LANGUAGE SQL IMMUTABLE AS $$
 SELECT CASE WHEN options->>'force_admission'='true' THEN 1000000
 WHEN options->>'priority'='high' THEN 2 WHEN options->>'priority'='interactive' THEN 1 ELSE 0 END
$$;
CREATE FUNCTION vrs_wait_priority(priority SMALLINT, forced BOOLEAN, arrived TIMESTAMPTZ)
RETURNS BIGINT LANGUAGE SQL STABLE AS $$
 SELECT (CASE WHEN forced THEN 1000000 ELSE priority END)::bigint + floor(greatest(0,extract(epoch FROM (now()-arrived)))/30)::bigint
$$;
