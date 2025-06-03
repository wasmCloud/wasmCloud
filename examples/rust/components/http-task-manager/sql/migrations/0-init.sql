-- Enable the uuid-ossp extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

BEGIN;

-- Basic table implementing tasks
CREATE TABLE IF NOT EXISTS tasks (
    id uuid NOT NULL DEFAULT uuid_generate_v1mc(),
    group_id text NOT NULL,
    status text NOT NULL DEFAULT 'pending',
    data_json jsonb NOT NULL,
    last_failed_at timestamptz,
    last_failure_reason text,
    lease_id uuid NOT NULL DEFAULT uuid_generate_v1mc(),
    leased_at timestamptz,
    lease_worker_id text,
    completed_at timestamptz,
    submitted_at timestamptz NOT NULL DEFAULT NOW(),
    last_updated_at timestamptz NOT NULL DEFAULT NOW(),
    CONSTRAINT check_status_enum CHECK (status IN ('pending', 'leased', 'completed', 'failed'))
);

COMMIT;
