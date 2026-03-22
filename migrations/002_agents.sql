CREATE TABLE agents (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    url         TEXT NOT NULL,
    version     TEXT NOT NULL DEFAULT '1.0.0',
    provider    JSONB DEFAULT '{}',
    capabilities JSONB DEFAULT '{}',
    default_input_modes  TEXT[] DEFAULT '{text}',
    default_output_modes TEXT[] DEFAULT '{text}',
    skills      JSONB NOT NULL DEFAULT '[]',
    security    JSONB DEFAULT '{}',
    card_json   JSONB NOT NULL,
    is_active   BOOLEAN DEFAULT true,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_agents_skills ON agents USING GIN (skills);
CREATE INDEX idx_agents_capabilities ON agents USING GIN (capabilities);
CREATE INDEX idx_agents_active ON agents (is_active) WHERE is_active = true;
