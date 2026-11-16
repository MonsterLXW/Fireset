use crate::Db;
use serde::Serialize;
use shared::Result;
use sqlx::FromRow;
use uuid::Uuid;

/// One NASA test condition row attached to a matched material.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct MatchedTest {
    pub condition_id: Uuid,
    pub paper_id: Uuid,
    pub ntrs_id: String,
    pub title: String,
    pub oxygen_concentration: Option<String>,
    pub pressure: Option<String>,
    pub airflow: Option<String>,
    /// The three text fields above as numbers (percent oxygen, kPa, cm/s), read by
    /// `units.rs`. A single value has min == max; null when it could not be read.
    pub oxygen_pct_min: Option<f64>,
    pub oxygen_pct_max: Option<f64>,
    pub pressure_kpa_min: Option<f64>,
    pub pressure_kpa_max: Option<f64>,
    pub airflow_cm_s_min: Option<f64>,
    pub airflow_cm_s_max: Option<f64>,
    pub outcome: String,
    pub outcome_note: Option<String>,
    pub source_page: Option<i32>,
    pub source_quote: Option<String>,
    pub verification_status: String,
}

/// Pointer to a DIFFERENT material that was tested. Never carries another
/// material's tests or outcome — only the name and a reason.
/// `fuel`/`note` are the names web/src/scan.rs reads (web/SCAN_API.md);
/// `material`/`why` are the same values under the backend's original keys.
#[derive(Debug, Clone, Serialize)]
pub struct ClosestTested {
    pub fuel: String,
    pub note: String,
    pub material: String,
    pub why: String,
}

/// One requested material. Key note for consumers: `matches`/`results` mirror
/// each other at the response level, and `query`/`material` mirror each other
/// here — one set is what the Burn Scan site parses, the other is the
/// backend's documented API.
#[derive(Debug, Clone, Serialize)]
pub struct MaterialMatch {
    pub query: String,
    pub material: String,
    /// All fuel_type spellings in our data that are this material.
    pub fuels: Vec<String>,
    pub matched_fuel: Option<String>,
    pub match_kind: String, // "exact" | "synonym" | "none"
    pub tests: Vec<MatchedTest>,
    pub closest: Option<ClosestTested>,
}

const TESTS_PER_MATERIAL: i64 = 50;

#[derive(Debug, FromRow)]
struct FuelCount {
    fuel: String,
    cnt: i64,
}

impl Db {
    /// Match one user-supplied material name against conditions.fuel_type.
    /// Exact match first, then the reviewed material_synonyms table.
    /// Rows with verification_status = 'rejected' are never returned.
    pub async fn match_material(
        &self,
        material: &str,
        oxygen_percent: Option<f64>,
    ) -> Result<MaterialMatch> {
        let clean = material.trim();
        let lowered = clean.to_lowercase();

        // Build the equivalence set for this query — one hop in each direction
        // through the reviewed synonym table only (real equivalences): the
        // query itself, the canonical(s) it is an alias of, and the aliases of
        // which it is the canonical. This is why cotton never bridges to paper
        // (both are canonicals of the alias "cellulose", not of each other).
        let mut keys: Vec<String> = vec![lowered.clone()];
        let canonicals: Vec<String> = sqlx::query_scalar(
            "SELECT lower(canonical) FROM material_synonyms WHERE alias = $1",
        )
        .bind(&lowered)
        .fetch_all(self.pool())
        .await?;
        let aliases: Vec<String> = sqlx::query_scalar(
            "SELECT lower(alias) FROM material_synonyms WHERE lower(canonical) = $1",
        )
        .bind(&lowered)
        .fetch_all(self.pool())
        .await?;
        for k in canonicals.iter().chain(aliases.iter()) {
            if !keys.iter().any(|x| x == k) {
                keys.push(k.clone());
            }
        }

        let fuels = self.matched_fuel_names(&keys).await?;
        let had_exact = fuels.iter().any(|f| f.fuel.to_lowercase() == lowered);
        let match_kind = if had_exact {
            "exact"
        } else if !fuels.is_empty() {
            "synonym"
        } else {
            "none"
        };

        if fuels.is_empty() {
            // Pointer only — never copy another material's tests or outcome.
            let closest = self.closest_tested_material(clean).await?;
            return Ok(MaterialMatch {
                query: clean.to_string(),
                material: clean.to_string(),
                fuels: vec![],
                matched_fuel: None,
                match_kind: "none".to_string(),
                tests: vec![],
                closest,
            });
        }

        let fuel_keys: Vec<String> = fuels.iter().map(|f| f.fuel.to_lowercase()).collect();
        let tests = self.tests_for_fuels(&fuel_keys, oxygen_percent).await?;
        // Most-tested fuel spelling wins as the representative matched_fuel.
        let matched_fuel = fuels.first().map(|f| f.fuel.clone());
        let fuel_names: Vec<String> = fuels.into_iter().map(|f| f.fuel).collect();

        Ok(MaterialMatch {
            query: clean.to_string(),
            material: clean.to_string(),
            fuels: fuel_names,
            matched_fuel,
            match_kind: match_kind.to_string(),
            tests,
            closest: None,
        })
    }

    /// fuel_type spellings (trimmed, as stored) whose lowercase equals one of
    /// `keys`, with counts, rejected rows excluded.
    async fn matched_fuel_names(&self, keys: &[String]) -> Result<Vec<FuelCount>> {
        let rows: Vec<FuelCount> = sqlx::query_as(
            "SELECT trim(fuel_type) AS fuel, count(*) AS cnt
             FROM conditions
             WHERE lower(trim(fuel_type)) = ANY($1)
               AND verification_status <> 'rejected'
             GROUP BY 1
             ORDER BY cnt DESC, fuel ASC",
        )
        .bind(keys)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    async fn tests_for_fuels(
        &self,
        keys: &[String],
        oxygen_percent: Option<f64>,
    ) -> Result<Vec<MatchedTest>> {
        let rows: Vec<MatchedTest> = sqlx::query_as(
            "SELECT c.id AS condition_id, c.paper_id, p.ntrs_id, p.title,
                    c.oxygen_concentration, c.pressure, c.airflow,
                    c.oxygen_pct_min::float8 AS oxygen_pct_min, c.oxygen_pct_max::float8 AS oxygen_pct_max,
                    c.pressure_kpa_min::float8 AS pressure_kpa_min, c.pressure_kpa_max::float8 AS pressure_kpa_max,
                    c.airflow_cm_s_min::float8 AS airflow_cm_s_min, c.airflow_cm_s_max::float8 AS airflow_cm_s_max,
                    c.outcome, c.outcome_note, c.source_page, c.source_quote,
                    c.verification_status
             FROM conditions c
             JOIN papers p ON p.id = c.paper_id
             WHERE lower(trim(c.fuel_type)) = ANY($1)
               AND c.verification_status <> 'rejected'
             ORDER BY (CASE WHEN $2::double precision IS NULL THEN NULL
                            ELSE abs((substring(c.oxygen_concentration
                                          FROM '([0-9]+([.][0-9]+)?)'))::numeric
                                     - $2::numeric)
                       END) ASC NULLS LAST,
                      c.confidence DESC, c.created_at ASC
             LIMIT $3",
        )
        .bind(keys)
        .bind(oxygen_percent)
        .bind(TESTS_PER_MATERIAL)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Pointer to the most name-similar tested material (character-bigram
    /// Dice similarity, computed in code so it needs no SQL extensions).
    /// Returns None when nothing resembles the query at all.
    async fn closest_tested_material(&self, material: &str) -> Result<Option<ClosestTested>> {
        let candidates: Vec<String> = sqlx::query_scalar(
            "SELECT trim(fuel_type) FROM conditions
             WHERE fuel_type IS NOT NULL AND fuel_type <> ''
               AND verification_status <> 'rejected'
             GROUP BY 1",
        )
        .fetch_all(self.pool())
        .await?;

        let mut best: Option<(f64, String)> = None;
        for c in candidates {
            let s = dice_bigram_similarity(&material.to_lowercase(), &c.to_lowercase());
            let better = match &best {
                Some((bs, bf)) => s > *bs || (s == *bs && c.to_lowercase() < bf.to_lowercase()),
                None => true,
            };
            if better {
                best = Some((s, c));
            }
        }

        let Some((score, fuel)) = best else { return Ok(None) };
        if score < 0.25 {
            return Ok(None);
        }
        let why = format!(
            "{fuel} is a different material, only the closest one we have tested for a name like “{}”.",
            material.trim()
        );
        Ok(Some(ClosestTested {
            fuel: fuel.clone(),
            note: why.clone(),
            material: fuel,
            why,
        }))
    }
}

/// Sørensen–Dice coefficient over character bigrams.
fn dice_bigram_similarity(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        chars.windows(2).map(|w| w.iter().collect()).collect()
    };
    let (ga, gb) = (bigrams(a), bigrams(b));
    if ga.is_empty() || gb.is_empty() {
        // 1-char strings fall back to equality
        return if a == b { 1.0 } else { 0.0 };
    }
    let total = ga.len() + gb.len();
    let mut gb = gb;
    let mut common = 0usize;
    for g in &ga {
        if let Some(pos) = gb.iter().position(|x| x == g) {
            gb.remove(pos);
            common += 1;
        }
    }
    (2 * common) as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::dice_bigram_similarity;

    #[test]
    fn similar_names_score_high() {
        assert!(dice_bigram_similarity("pmma", "acryl pmma") > 0.4);
        assert!(dice_bigram_similarity("cotton", "cotton") == 1.0);
    }

    #[test]
    fn unrelated_names_score_low() {
        assert!(dice_bigram_similarity("vectran", "pmma") < 0.25);
        assert!(dice_bigram_similarity("steel", "nylon") < 0.25);
    }
}
