-- mutant and execution are separate tables: a green or attributed batch
-- attaches several mutants to the one execution that settled them. A survived
-- mutant with no execution_id was never executed by any test.

CREATE TABLE IF NOT EXISTS mutant (
    id INTEGER PRIMARY KEY,
    file TEXT NOT NULL,
    line INTEGER NOT NULL,
    byte_start INTEGER NOT NULL,
    byte_end INTEGER NOT NULL,
    original TEXT NOT NULL,
    replacement TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    execution_id INTEGER
);

CREATE TABLE IF NOT EXISTS execution (
    id INTEGER PRIMARY KEY,
    duration_ms INTEGER NOT NULL,
    exit_code INTEGER,
    failed_tests TEXT NOT NULL DEFAULT ''
);
