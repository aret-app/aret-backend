-- Account deletion (aret-backend#13).

-- Users whose data was purged after `user.deleted`. Their session JWTs stay valid for up to
-- 10 minutes, so saving a track checks this table: nothing new can appear after the purge.
CREATE TABLE deleted_users (
    user_id     uuid PRIMARY KEY,
    purged_at   timestamptz NOT NULL DEFAULT now()
);

-- Transactional outbox (crates/aret-events/src/outbox.rs): `purge.completed`
CREATE TABLE outbox (
    id            uuid PRIMARY KEY,
    subject       text NOT NULL,
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    published_at  timestamptz
);
CREATE INDEX outbox_unpublished_idx ON outbox (created_at, id) WHERE published_at IS NULL;
