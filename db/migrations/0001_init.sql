-- Initial schema for Audify.
-- Status/state columns are TEXT, mapped to typed enums in `audify-core`.

-- Extracted source material, deduplicated by content hash.
CREATE TABLE documents (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type  TEXT NOT NULL,                 -- 'url' | 'text'
    source_ref   TEXT NOT NULL,                 -- the URL, or a label for raw text
    content_hash TEXT NOT NULL UNIQUE,          -- dedup key (sha256 of source)
    title        TEXT,
    struct_json  JSONB NOT NULL,                -- the extracted Document tree
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One rendered episode per (document, voice).
CREATE TABLE episodes (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id  UUID NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    status       TEXT NOT NULL DEFAULT 'pending',
    voice_id     TEXT NOT NULL,
    char_count   INTEGER NOT NULL DEFAULT 0,
    duration_sec INTEGER,
    audio_path   TEXT,
    est_cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_episodes_document ON episodes (document_id);
CREATE INDEX idx_episodes_created ON episodes (created_at DESC);

-- Per-section audio, enabling parallel synthesis and resume-on-retry.
CREATE TABLE chunks (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_id UUID NOT NULL REFERENCES episodes (id) ON DELETE CASCADE,
    idx        INTEGER NOT NULL,
    text       TEXT NOT NULL,
    audio_path TEXT,
    status     TEXT NOT NULL DEFAULT 'pending',
    UNIQUE (episode_id, idx)
);

-- Async processing jobs with lease-based reaping (locked_until).
CREATE TABLE processing_jobs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_id   UUID NOT NULL REFERENCES episodes (id) ON DELETE CASCADE,
    state        TEXT NOT NULL DEFAULT 'pending',
    attempts     INTEGER NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_jobs_state ON processing_jobs (state);
