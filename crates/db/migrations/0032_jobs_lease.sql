-- A claimed job records when it was claimed, so a worker that dies with
-- it -- a crash, a restart mid-run -- does not leave it 'running' for
-- ever. Nothing recovered such jobs before: 'running' was a terminal
-- state the moment the process holding it went away.
ALTER TABLE jobs ADD COLUMN claimed_at TIMESTAMPTZ;
