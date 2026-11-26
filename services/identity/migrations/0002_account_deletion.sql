-- Account deletion (aret-backend#13). A deleted account keeps its row as a tombstone (ids and
-- times only), so the catch-up loop never announces it again and purges can be tracked.

ALTER TABLE users
    ADD COLUMN deleted_at        timestamptz,   -- the user asked to delete the account
    ADD COLUMN kratos_deleted_at timestamptz;   -- identity (email, password, sessions) gone from Kratos

-- Deletions whose Kratos call still has to succeed (retried by the catch-up loop)
CREATE INDEX users_kratos_pending_idx ON users (deleted_at)
    WHERE deleted_at IS NOT NULL AND kratos_deleted_at IS NULL;

-- Which services confirmed they deleted a user's data (`aret.purge.completed`)
CREATE TABLE purges (
    user_id       uuid NOT NULL,
    service       text NOT NULL,
    completed_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, service)
);
