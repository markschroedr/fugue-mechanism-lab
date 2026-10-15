CREATE TABLE society_world_run (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE TABLE society_world_submission (
    instance_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES society_world_run(id),
    actor_id TEXT NOT NULL,
    submission_id TEXT NOT NULL,
    input_json TEXT NOT NULL CHECK(json_valid(input_json)),
    receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json)),
    reward INTEGER NOT NULL CHECK(reward IN (0,1)),
    ledger_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY(instance_id, actor_id, submission_id)
);
CREATE TABLE society_world_termination (
    instance_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES society_world_run(id),
    actor_id TEXT NOT NULL,
    tripwire TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY(instance_id, actor_id)
);
CREATE TRIGGER society_world_run_no_update BEFORE UPDATE ON society_world_run BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER society_world_run_no_delete BEFORE DELETE ON society_world_run BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER society_world_submission_no_update BEFORE UPDATE ON society_world_submission BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER society_world_submission_no_delete BEFORE DELETE ON society_world_submission BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER society_world_termination_no_update BEFORE UPDATE ON society_world_termination BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER society_world_termination_no_delete BEFORE DELETE ON society_world_termination BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
