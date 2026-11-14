-- FLAME research platform (fire-intel): initial schema
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- ---------- papers (NTRS publications) ----------
CREATE TABLE papers (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    ntrs_id           text NOT NULL UNIQUE,          -- NTRS citation id (numeric submission id or uuid)
    title             text NOT NULL DEFAULT '',
    abstract          text NOT NULL DEFAULT '',
    authors           jsonb NOT NULL DEFAULT '[]',   -- ["Family, Given", ...]
    publication_date  text,                          -- ISO date or partial, as reported by NTRS
    doi               text,
    ntrs_url          text NOT NULL DEFAULT '',
    pdf_url           text,
    pdf_path          text,                          -- local storage path once downloaded
    pub_type          text NOT NULL DEFAULT '',
    fulltext          text NOT NULL DEFAULT '',      -- normalized extracted text
    pages_total       int,
    discovery_query   text NOT NULL DEFAULT '',
    status            text NOT NULL DEFAULT 'discovered'
        CHECK (status IN ('discovered','metadata','downloaded','extracted','chunked',
                          'matched','extracted_claims','complete','failed')),
    error             text,
    retry_count       int NOT NULL DEFAULT 0,
    first_seen_at     timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX papers_status_idx ON papers(status);

-- ---------- experiments ----------
CREATE TABLE experiments (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    acronym            text NOT NULL UNIQUE,
    name               text NOT NULL DEFAULT '',
    aliases            jsonb NOT NULL DEFAULT '[]',
    description        text NOT NULL DEFAULT '',
    source             text NOT NULL DEFAULT 'seed',  -- seed | sheet | discovered
    verification_status text NOT NULL DEFAULT 'seed'
        CHECK (verification_status IN ('seed','reviewed','verified')),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE experiment_papers (
    experiment_id  uuid NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
    paper_id       uuid NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    match_method   text NOT NULL DEFAULT 'acronym' CHECK (match_method IN ('acronym','sheet','ai','manual')),
    confidence     real NOT NULL DEFAULT 1.0,
    created_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (experiment_id, paper_id)
);

-- ---------- chunks (searchable, page-tracked, embedded) ----------
CREATE TABLE paper_chunks (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    paper_id          uuid NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    ordinal           int NOT NULL,
    page_start        int,                    -- 1-based page of PDF the chunk starts on
    page_end          int,
    char_start        int NOT NULL,
    char_end          int NOT NULL,
    text              text NOT NULL,
    text_tsv          tsvector GENERATED ALWAYS AS (to_tsvector('english', text)) STORED,
    embedding         vector(1024),           -- dim must match VLLM_EMBED_DIM; NULL until served
    embedding_model   text,
    embedding_status  text NOT NULL DEFAULT 'pending'
        CHECK (embedding_status IN ('pending','ready','unavailable','failed')),
    UNIQUE (paper_id, ordinal)
);
CREATE INDEX chunks_tsv_idx ON paper_chunks USING gin (text_tsv);
CREATE INDEX chunks_embedding_idx ON paper_chunks USING hnsw (embedding vector_cosine_ops);

-- ---------- experimental conditions (per paper, optionally per experiment) ----------
CREATE TABLE conditions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    paper_id            uuid NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    experiment_id       uuid REFERENCES experiments(id) ON DELETE SET NULL,
    fuel_type           text,
    fuel_geometry       text,
    oxygen_concentration text,
    pressure            text,
    airflow             text,
    sensors             jsonb,                     -- ["camera", ...]
    objective           text,
    source_page         int,                       -- 1-based, NULL when unknown
    source_quote        text,
    provenance_kind     text NOT NULL DEFAULT 'extracted'
        CHECK (provenance_kind IN ('reported','extracted','inferred','unknown')),
    confidence          real NOT NULL DEFAULT 0.0, -- 0..1 extraction confidence
    verification_status text NOT NULL DEFAULT 'pending'
        CHECK (verification_status IN ('pending','verified','corrected','rejected')),
    correction_note     text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

-- ---------- findings (claims, linked to source page, classified) ----------
CREATE TABLE findings (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    paper_id            uuid NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    experiment_id       uuid REFERENCES experiments(id) ON DELETE SET NULL,
    text                text NOT NULL,
    category            text CHECK (category IN ('prevention','detection','suppression','other') OR category IS NULL),
    source_page         int,
    source_quote        text,
    provenance_kind     text NOT NULL DEFAULT 'extracted'
        CHECK (provenance_kind IN ('reported','extracted','inferred','unknown')),
    confidence          real NOT NULL DEFAULT 0.0,
    verification_status text NOT NULL DEFAULT 'pending'
        CHECK (verification_status IN ('pending','verified','corrected','rejected')),
    correction_note     text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX findings_category_idx ON findings(category);

-- ---------- pipeline jobs (retries, incremental, status tracking) ----------
CREATE TABLE pipeline_jobs (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind          text NOT NULL,                -- discover|metadata|download|extract_text|chunk|match|extract_claims|embed
    paper_id      uuid REFERENCES papers(id) ON DELETE CASCADE,
    payload       jsonb NOT NULL DEFAULT '{}',
    status        text NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','done','failed','skipped')),
    attempts      int NOT NULL DEFAULT 0,
    max_attempts  int NOT NULL DEFAULT 3,
    error         text,
    available_at  timestamptz NOT NULL DEFAULT now(),
    started_at    timestamptz,
    finished_at   timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX pipeline_jobs_due_idx ON pipeline_jobs(status, available_at) WHERE status IN ('queued','failed');

-- ---------- Google Sheet import bookkeeping ----------
CREATE TABLE sheet_imports (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sheet_url   text NOT NULL,
    total_rows  int NOT NULL DEFAULT 0,
    ok_rows     int NOT NULL DEFAULT 0,
    errors      jsonb NOT NULL DEFAULT '[]',
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- ---------- conversations (research assistant history) ----------
CREATE TABLE conversations (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    title       text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE messages (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id  uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role             text NOT NULL CHECK (role IN ('user','assistant')),
    content          text NOT NULL,
    citations        jsonb NOT NULL DEFAULT '[]',
    created_at       timestamptz NOT NULL DEFAULT now()
);

-- ---------- autonomous AI workflows ----------
CREATE TABLE workflow_runs (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    goal          text NOT NULL,
    status        text NOT NULL DEFAULT 'running'
        CHECK (status IN ('running','done','failed','cancelled')),
    max_steps     int NOT NULL DEFAULT 24,
    result_summary text,
    error          text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    finished_at    timestamptz
);
CREATE TABLE workflow_steps (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id      uuid NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    seq         int NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('thought','tool_call','tool_result','final')),
    tool_name   text,
    input       jsonb,
    output      jsonb,
    ok          boolean,
    error       text,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- ---------- AI call audit ----------
CREATE TABLE ai_call_log (
    id          bigserial PRIMARY KEY,
    route       text NOT NULL,                  -- messages|embeddings|models
    model       text,
    http_status int,
    ok          boolean NOT NULL,
    in_tokens   int,
    out_tokens  int,
    latency_ms  int,
    error       text,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- ---------- seed experiments (from the program list; descriptions minimal & factual) ----------
INSERT INTO experiments (acronym, name, aliases, description, source) VALUES
 ('BASS',    'Burn and Splatter Suppression',                      '["BASS-I"]', 'Microgravity flammability and fire suppression experiments on the ISS.', 'seed'),
 ('BASS-II', 'BASS-Flammability Experiment II',                    '["BASS Flammability"]', 'Second BASS flammability experiment series on the ISS.', 'seed'),
 ('SAFFIRE I',  'Saffire Fireward Experiment I',   '["SAFFIRE-I","Saffire I"]', 'First Saffire outward-flame experiment with a cylindrical fuel array on the ISS.', 'seed'),
 ('SAFFIRE II', 'Saffire Fireward Experiment II',  '["SAFFIRE-II","Saffire II"]', 'Second Saffire experiment series examining buoyant and opposed-flow flames in microgravity.', 'seed'),
 ('SAFFIRE III','Saffire Fireward Experiment III', '["SAFFIRE-III","Saffire III"]','Third Saffire experiment series studying burning rate and flame spread of arrays.', 'seed'),
 ('SAME',    'Combustion Integrated Rack Smoke Experiment',        '["Smoke Masking"]', 'Measurement of soot/smoke optical properties in microgravity.', 'seed'),
 ('SAME-R',  'Smoke Aggregation and Modification Experiment - Retrieval','["SAME-Retrieval"]','Follow-up to SAME examining soot aggregation; hardware returned for analysis.', 'seed'),
 ('FLEX',    'Flammability Experiment',                            '["Fireward Experiment"]', 'Opposed-flow flame extinguishment under microgravity with variable flow and oxygen.', 'seed'),
 ('FLEX-2',  'Flammability Experiment 2',                          '["FLEX II"]', 'Second FLEX series on flame spread and extinction in variable gravity.', 'seed'),
 ('CFI',     'Combustion Integration Facility experiments',        '["Combustion Integration Facility"]','Experiments performed in the ISS Combustion Integrated Rack.', 'seed'),
 ('ACME',    ' Aerosol, Combustion, and Materials Experiment',     '["ACME-1","ACME-2","ACME-3","ACME-4","ACME-5","ACME-6","ACME6"]','Six-run series measuring smoke from burning solids in microgravity.', 'seed'),
 ('SPICE',   'Spacefire Propagation and Ingress Characterization Experiment','["SPICE-I","SPICE-II"]','Propagation of flame along wires in microgravity (single- and two-wire configurations).', 'seed'),
 ('SLICE',   'Structure and Lift-off In Controlled Experiment',    '["SLICE-I","SLICE-II"]','Turbulent jet flame structure and lift-off under controlled gravity levels.', 'seed');
