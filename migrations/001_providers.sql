CREATE TABLE providers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    provider_type TEXT NOT NULL,
    base_url    TEXT NOT NULL,
    api_key_encrypted BYTEA,
    models      JSONB NOT NULL DEFAULT '[]',
    cost_per_input_token  DOUBLE PRECISION,
    cost_per_output_token DOUBLE PRECISION,
    rate_limit_rpm INTEGER,
    priority    INTEGER DEFAULT 0,
    weight      INTEGER DEFAULT 1,
    is_active   BOOLEAN DEFAULT true,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_providers_active ON providers (is_active) WHERE is_active = true;
