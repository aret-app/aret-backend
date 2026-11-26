-- identity-svc (aret-backend#12): accounts it has announced, and its event outbox.
-- Schema `identity`, owned by role `identity` (dev/postgres/setup.sh).

-- Every Kratos identity identity-svc has seen. Ids only: personal data stays in Kratos (spec §10).
CREATE TABLE users (
    id          uuid PRIMARY KEY,                -- Kratos identity id
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- Transactional outbox (crates/aret-events/src/outbox.rs)
CREATE TABLE outbox (
    id            uuid PRIMARY KEY,              -- event id, also the JetStream message id
    subject       text NOT NULL,
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    published_at  timestamptz                    -- NULL until the relay published it
);
CREATE INDEX outbox_unpublished_idx ON outbox (created_at, id) WHERE published_at IS NULL;
