-- Numbers for the flammability map (oxygen against pressure, marked by outcome).
--
-- oxygen_concentration, pressure and airflow stay as they are: free text, the record of
-- what the paper said. These six columns hold the same values as numbers in fixed units,
-- so they can be plotted and filtered:
--   oxygen_pct_*    percent oxygen by volume (mole fraction x 100)
--   pressure_kpa_*  absolute pressure in kPa
--   airflow_cm_s_*  flow speed in cm/s
-- A single stated value has min = max. A stated range has min < max. NULL means the paper
-- did not state it, or the text could not be read as one unambiguous value or range.
-- They are filled by the parser in crates/db/src/units.rs (never by the model): on every
-- new extraction, on a corrected row, and for existing rows by `fire-intel backfill-numeric`.

ALTER TABLE conditions
    ADD COLUMN IF NOT EXISTS oxygen_pct_min numeric,
    ADD COLUMN IF NOT EXISTS oxygen_pct_max numeric,
    ADD COLUMN IF NOT EXISTS pressure_kpa_min numeric,
    ADD COLUMN IF NOT EXISTS pressure_kpa_max numeric,
    ADD COLUMN IF NOT EXISTS airflow_cm_s_min numeric,
    ADD COLUMN IF NOT EXISTS airflow_cm_s_max numeric;

-- The public (publishable-key) read role must see the new columns like the other
-- condition columns. These roles exist only in Supabase; skip silently on a plain
-- local Postgres.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN
        EXECUTE 'GRANT SELECT (oxygen_pct_min, oxygen_pct_max, pressure_kpa_min, pressure_kpa_max, airflow_cm_s_min, airflow_cm_s_max) ON public.conditions TO anon';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'authenticated') THEN
        EXECUTE 'GRANT SELECT (oxygen_pct_min, oxygen_pct_max, pressure_kpa_min, pressure_kpa_max, airflow_cm_s_min, airflow_cm_s_max) ON public.conditions TO authenticated';
    END IF;
END $$;
