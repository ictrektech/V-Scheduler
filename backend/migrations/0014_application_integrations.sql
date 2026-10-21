-- Opt-in lifecycle contract. Existing auto-bound clients keep their protocol.
CREATE TABLE vrs_integrations (
 app_id TEXT PRIMARY KEY,
 manifest JSONB NOT NULL,
 revision BIGINT NOT NULL DEFAULT 1,
 heartbeat TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE vrs_stop_requests (
 permit_id TEXT PRIMARY KEY REFERENCES vrs_permits(id),
 app_id TEXT NOT NULL,
 nonce TEXT NOT NULL,
 requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 confirmed_at TIMESTAMPTZ
);
CREATE INDEX vrs_stop_requests_pending ON vrs_stop_requests(app_id) WHERE confirmed_at IS NULL;
