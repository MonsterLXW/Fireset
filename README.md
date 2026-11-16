# Fireset — fire-intel backend

Backend for **Flame in Freefall** / **Burn Scan** (NASA Space Apps 2026): ingests NASA NTRS
combustion papers, extracts experimental conditions and test outcomes with a self-hosted model,
answers questions with citations, and serves the data the dashboard reads.

Per `AGENTS.md` (kavintx/nasaspaceappschallenge), the source of truth for `crates/`,
`migrations/` and the root `Cargo.*` is **this repo**. The dashboard lives in the team repo's
`web/` and reads the same database through Supabase.

## Run

```sh
cp .env.example .env   # fill in secrets — .env is gitignored
cargo run -p api       # http://localhost:8080 (routes under /api/v1)
```

On the Windows dev hosts the native toolchain is blocked (Smart App Control); build and test
in Docker instead:

```sh
docker run --rm -e CARGO_HOME=/cargo -v "$PWD:/workspace" -v fireset-target:/workspace/target \
  -w /workspace rust:1-bookworm cargo test --workspace
```

The API applies `migrations/` automatically at startup (sqlx). Against Supabase, migration 1 is
already recorded in `_sqlx_migrations`, so later migrations must apply cleanly on top of it.

## Environment

| Var | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | local fire-intel DB | Postgres URL. Use the Supabase **Session pooler** string (port 5432, `postgres.<ref>` user) — the backend uses prepared statements, which the Transaction pooler breaks. |
| `API_AUTH_TOKEN` | — (required, ≥24 chars) | bearer token for every `/api/v1/*` route |
| `BIND_ADDR` | `0.0.0.0:8080` | listen address |
| `AI_PROVIDER` | `vllm` | must stay `vllm` — text inference is self-hosted only |
| `VLLM_BASE_URL` | tailscale vLLM | Anthropic-protocol server for text |
| `VLLM_API_KEY` | empty | sent as `x-api-key` when set |
| `VLLM_MODEL` | `qwen3.8-flash-next` | text model (extraction, answers) |
| `VLLM_TIMEOUT_SECONDS` / `VLLM_MAX_RETRIES` | 180 / 3 | |
| `VISION_BASE_URL` | = `VLLM_BASE_URL` | **only set when the text model can't take images.** Points `/ai/scan` at a vision-capable Anthropic-protocol server. |
| `VISION_MODEL` | = `VLLM_MODEL` | vision model id (our vLLM accepts image blocks, so the default works) |
| `VISION_API_KEY` | = `VLLM_API_KEY` | vision server key. **Keys live in env only — never in the repo, never in logs.** |
| `SCAN_RATE_LIMIT_PER_MINUTE` | `20` | per-token limit on `POST /ai/scan` |
| `VLLM_EMBED_MODEL` / `VLLM_EMBED_DIM` | same host / 1024 | embeddings (`/v1/embeddings`) |
| `PDF_STORE_DIR` | `/data/pdfs` | downloaded PDFs |
| `NTRS_BASE_URL` | `https://ntrs.nasa.gov` | |
| `MAX_CONCURRENT_INGEST` | `2` | |
| `ALLOWED_ORIGINS` | localhost:5173 | CORS list |

## New endpoints

### `POST /api/v1/ai/scan` — Burn Scan photo → materials

Bearer auth, same as the other `/ai` routes. Two request forms (same limits and checks):

- **multipart/form-data** — `image`: JPEG/PNG/WebP (checked by magic bytes), **max 5 MB**
  (413 over; 400 for non-images); `mode`: `item` (default) or `label` (photo is a
  fiber-content/materials label: read the text). `mode` may also ride in the query string.
- **raw body** — the photo bytes as the whole body with an `image/...` content type and
  `?mode=` in the query string. This is what the dashboard's server-side proxy sends
  (`web/.env` `VISION_PROVIDER=backend`), so it never re-encodes the photo.

The image is processed in memory and discarded: never stored, never logged. If a person is in
the photo, only the item is described. Materials are plain generic names (brand names are
converted or refused); `share` is `null` whenever the blend share would be an
unsupported guess, and `unclear` is `true` for blurry/dark/no-clear-item photos.

```sh
curl -s -X POST http://localhost:8080/api/v1/ai/scan \
  -H "authorization: Bearer $API_AUTH_TOKEN" \
  -F "image=@shirt.jpg" -F "mode=item"
```

The response carries both key sets: `web/SCAN_API.md` names (what the Burn Scan site parses)
and the backend's original names, same values:

```json
{
  "item": "T-Shirt", "confidence": 0.95,
  "unclear": false, "needs_better_photo": false, "message": null,
  "model": "vllm:qwen3.8-flash-next",
  "note": "Fiber blend is a visual estimate; scan the label for exact content.",
  "parts": [
    { "name": "body", "part": "body", "materials": [
      { "material": "cotton", "share": 100, "share_percent": 100, "confidence": 0.99 }
    ] }
  ]
}
```

Label mode:

```sh
curl -s -X POST http://localhost:8080/api/v1/ai/scan \
  -H "authorization: Bearer $API_AUTH_TOKEN" \
  -F "image=@label.jpg" -F "mode=label"
```

### `POST /api/v1/materials/match` — materials → NASA test outcomes

Bearer auth, JSON body. **No model call** — matching is exact string plus the reviewed
`material_synonyms` table (acrylic = PMMA = plexiglass, Kapton = polyimide, Teflon = PTFE,
cellulose ~ cotton/paper, …), all in SQL.

```sh
curl -s -X POST http://localhost:8080/api/v1/materials/match \
  -H "authorization: Bearer $API_AUTH_TOKEN" -H "content-type: application/json" \
  -d '{"materials": ["cotton", "acrylic", "vectran"], "oxygen_percent": 30}'
```

Response: `{ "matches": [...], "results": [...] }` — the same array under both keys
(`matches` is what the Burn Scan site parses, `results` the backend's original name). Per
material: `match_kind` `exact` | `synonym` | `none`; `matched_fuel` is the fuel_type string
in our data (or null); `fuels` lists every fuel_type spelling that is this material (the
synonym expansion is symmetric one-hop: acrylic → PMMA + Acrylic, but cellulose never
bridges cotton to paper); `tests` carry the condition fields (the text `oxygen_concentration`,
`pressure`, `airflow` and their six numeric fields, see "Conditions: numbers" below) plus `outcome`, `outcome_note`,
`source_page`, `source_quote` and `verification_status` (`rejected` rows are excluded);
`oxygen_percent` only orders tests by closeness. When `match_kind` is `none`, `closest`
(`{fuel, note}` = `{material, why}`) is a **pointer to a different material** — that
material's tests and outcome are never copied onto the unmatched one.

### Accounts (`POST/GET /api/v1/auth/*`)

Public routes (no site bearer token) backing the dashboard's Account page:
`signup` (email + password ≥10 + optional name), `login`, `me`, `logout`.
Passwords are argon2id-hashed; a login returns a random session token which
the dashboard keeps in an HttpOnly cookie and replays as
`Authorization: Bearer <session token>`. Only the token's SHA-256 is stored,
sessions expire after 30 days, and `users`/`sessions` are invisible to the
public read roles. Signup/login are rate-limited per email.

## Conditions: test outcomes

`conditions.outcome` ∈ `ignited | sustained_flame | self_extinguished | no_ignition | unknown`
(default `unknown`), plus `outcome_note` (one sentence in the paper's own terms) and
`outcome_source_page` / `outcome_source_quote`. The extraction prompt asks for these on every
condition and **must not infer**: when the paper doesn't say what happened, the outcome stays
`unknown`. Outcomes are returned everywhere conditions are returned, and the public read role
can select them.

To backfill papers extracted before this existed, re-run extraction (pending/rejected rows are
replaced; human-verified rows are kept):

```sh
curl -s -X POST http://localhost:8080/api/v1/ai/extract/batch \
  -H "authorization: Bearer $API_AUTH_TOKEN" -H "content-type: application/json" \
  -d '{"limit": 5}'
```

For a backfill run, pass explicit `{"paper_ids": ["<uuid>", ...]}` sections (8 at a time run
well in parallel — disjoint ids mean the batches never race over paper selection) instead of
one `limit: 44` request.

## Conditions: numbers for oxygen, pressure and airflow

`oxygen_concentration`, `pressure` and `airflow` are free text: the record of what the paper
said. Migration `0005_numeric_conditions.sql` adds the same values as numbers in fixed units,
so tests can be plotted and filtered (the dashboard's flammability map):

| Columns | Unit | Read from |
|---|---|---|
| `oxygen_pct_min`, `oxygen_pct_max` | percent oxygen | `oxygen_concentration` |
| `pressure_kpa_min`, `pressure_kpa_max` | kPa, absolute | `pressure` |
| `airflow_cm_s_min`, `airflow_cm_s_max` | cm/s | `airflow` |

All six are nullable `numeric`. A single value has min = max; a stated range has min < max.
They are returned as JSON numbers (or `null`) everywhere conditions are returned:
`POST /api/v1/materials/match` (on each test), `GET /api/v1/papers/{id}/extractions`,
`PATCH /api/v1/conditions/{id}` and the extraction responses. No existing key or field changed.
The public read roles can select them.

The numbers come from one parser, `crates/db/src/units.rs`, never from the model:

- **Oxygen:** `30%`, `21-40% O2`, `21 vol%`, `0.21 mole fraction`. In `21% O2 and 79% N2` only
  the oxygen figure counts. A bare `0.17` has no unit and stays null.
- **Pressure:** psi, psia, kPa, MPa, Pa, atm, bar, mbar, torr, mmHg, converted to kPa. Gauge
  pressure (`psig`) is not absolute and stays null. `1 atm (101.3 kPa)` is one value.
- **Airflow:** cm/s, mm/s, m/s, converted to cm/s. `quiescent` states no number and stays null.
- **Null on purpose:** no unit, a bound (`below 16%`, `up to 30 cm/s`, `< 1 cm/s`), a value that
  varied, or several separate conditions in one row (`15%, 18% and 21%`).
- **Oxygen as a partial pressure** (`21.3 kPa O2`) is not a percentage. It becomes one only when
  the same row states a single total pressure.

Where they are filled:

- **New extractions:** on insert, from the text the model returned (`replace_paper_extractions`).
- **Corrections:** `PATCH /api/v1/conditions/{id}` re-reads all three when any of the three texts
  is changed.
- **Existing rows:** `fire-intel backfill-numeric`. It fills only columns that are still NULL, so
  it can be run again at any time. `--dry-run` writes no rows and prints how many would be
  filled and up to 20 texts it could not read.

```sh
cargo run -p cli -- backfill-numeric --dry-run
cargo run -p cli -- backfill-numeric
```

Note that every `fire-intel` command, including `--dry-run`, connects through `Db::connect`,
which applies pending migrations first. Against a database that does not have migration 5 yet,
the dry run adds the six (empty) columns and then writes nothing else.

## Ground rules

- Never call `POST /api/v1/pipeline/nasa/scrape` — it overwrites the 13 curated experiments
  from one generic page.
- No invented data: no made-up outcomes, sources, or NTRS IDs. Missing is `null` / `unknown`.
- Do not change the `GroundedAnswer` shape or the `/ask`-related and
  `/api/v1/ai/habitat/analyze` routes — the dashboard depends on them.
- No secrets in the repo; `.env` stays local.
