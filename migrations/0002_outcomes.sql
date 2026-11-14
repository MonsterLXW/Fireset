-- Test outcomes on extracted conditions (Burn Scan).
-- outcome = what happened to the fuel in that test, as stated by the paper.
-- 'unknown' is the default and must be used when the paper does not say.
-- outcome_note keeps the paper's own wording in one sentence; the outcome
-- source columns point at where it was read (kept separate from the
-- condition-level source_page/source_quote, which describe the setup).

ALTER TABLE conditions
    ADD COLUMN IF NOT EXISTS outcome text NOT NULL DEFAULT 'unknown'
        CHECK (outcome IN ('ignited','sustained_flame','self_extinguished',
                           'no_ignition','unknown')),
    ADD COLUMN IF NOT EXISTS outcome_note text,
    ADD COLUMN IF NOT EXISTS outcome_source_page int,
    ADD COLUMN IF NOT EXISTS outcome_source_quote text;

CREATE INDEX IF NOT EXISTS conditions_outcome_idx ON conditions(outcome);
CREATE INDEX IF NOT EXISTS conditions_fuel_type_idx ON conditions(lower(trim(fuel_type)));

-- The public (publishable-key) read role must see the new columns like the
-- other condition columns. These roles exist only in Supabase; skip silently
-- on a plain local Postgres.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN
        EXECUTE 'GRANT SELECT (outcome, outcome_note, outcome_source_page, outcome_source_quote) ON public.conditions TO anon';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'authenticated') THEN
        EXECUTE 'GRANT SELECT (outcome, outcome_note, outcome_source_page, outcome_source_quote) ON public.conditions TO authenticated';
    END IF;
END $$;
