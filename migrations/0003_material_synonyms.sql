-- Reviewed material synonym table for /materials/match.
-- One row per (alias -> canonical material) pair; an alias may list several
-- canonical materials (e.g. cellulose -> cotton and paper).
-- These are REAL equivalences only: brand names to their generic material,
-- or alternate generic names for the identical polymer. No near-relatives:
-- "closest tested" pointers are computed separately and never copy tests.
-- alias is matched case-insensitively against the user's input; canonical is
-- compared (case-insensitively, trimmed) against conditions.fuel_type.

CREATE TABLE IF NOT EXISTS material_synonyms (
    alias     text NOT NULL,
    canonical text NOT NULL,
    PRIMARY KEY (alias, canonical)
);

INSERT INTO material_synonyms (alias, canonical) VALUES
    -- acrylic / PMMA
    ('acrylic',                  'PMMA'),
    ('plexiglass',               'PMMA'),
    ('plexiglas',                'PMMA'),
    ('perspex',                  'PMMA'),
    ('polymethyl methacrylate',  'PMMA'),
    ('poly(methyl methacrylate)','PMMA'),
    -- polyimide
    ('kapton',                   'polyimide'),
    ('upeks',                    'polyimide'),
    -- PTFE
    ('teflon',                   'PTFE'),
    ('fluon',                    'PTFE'),
    -- cellulose-based
    ('cellulose',                'cotton'),
    ('cellulose',                'paper'),
    -- polyester / PET
    ('pet',                      'polyethylene terephthalate'),
    ('pete',                     'polyethylene terephthalate'),
    -- elastane (brand and regional generic names)
    ('lycra',                    'elastane'),
    ('spandex',                  'elastane'),
    ('elastane',                 'spandex'),
    -- aramid (brand names to generic)
    ('kevlar',                   'aramid'),
    ('nomex',                    'aramid'),
    ('technora',                 'aramid'),
    -- polycarbonate
    ('lexan',                    'polycarbonate'),
    ('makrolon',                 'polycarbonate')
ON CONFLICT (alias, canonical) DO NOTHING;

-- Read-only exposure for the public (Supabase) roles; skipped on plain local Postgres.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN
        EXECUTE 'ALTER TABLE public.material_synonyms ENABLE ROW LEVEL SECURITY';
        EXECUTE 'GRANT SELECT ON public.material_synonyms TO anon';
        IF NOT EXISTS (SELECT 1 FROM pg_policies WHERE tablename = 'material_synonyms' AND policyname = 'public read') THEN
            EXECUTE 'CREATE POLICY "public read" ON public.material_synonyms FOR SELECT TO anon, authenticated USING (true)';
        END IF;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'authenticated') THEN
        EXECUTE 'GRANT SELECT ON public.material_synonyms TO authenticated';
    END IF;
END $$;
