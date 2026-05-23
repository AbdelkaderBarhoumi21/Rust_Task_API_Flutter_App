CREATE TYPE device_platform AS ENUM ('android', 'ios', 'web');

CREATE TABLE devices (
    id           UUID PRIMARY KEY,
    token        TEXT NOT NULL UNIQUE,
    platform     device_platform NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_devices_token ON devices (token);
