-- API high priority is 2. Preserve existing waiters while allowing the same
-- three request/task priorities already accepted by the controller and gateway.
ALTER TABLE vrs_inference_waiters DROP CONSTRAINT vrs_inference_waiters_priority_check;
ALTER TABLE vrs_inference_waiters ADD CONSTRAINT vrs_inference_waiters_priority_check CHECK (priority BETWEEN 0 AND 2);
