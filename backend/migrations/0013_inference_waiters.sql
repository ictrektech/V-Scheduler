-- Admission hints only: never store inference payloads or replay work here.
CREATE TABLE vrs_inference_waiters (
 app TEXT NOT NULL, id TEXT NOT NULL, executor TEXT NOT NULL,
 node TEXT NOT NULL REFERENCES vrs_nodes(id), priority SMALLINT NOT NULL CHECK(priority BETWEEN 0 AND 1),
 arrived_at TIMESTAMPTZ NOT NULL DEFAULT now(), valid_until TIMESTAMPTZ NOT NULL,
 PRIMARY KEY(app,id)
);
CREATE INDEX vrs_inference_waiters_node ON vrs_inference_waiters(node,valid_until);
