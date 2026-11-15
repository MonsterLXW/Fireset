use crate::{ApiError, ApiResult, AppState};
use axum::{extract::State, Json};
use db::repo_materials::MaterialMatch;
use serde::Deserialize;
use shared::Error;

const MAX_MATERIALS: usize = 25;

#[derive(Debug, Deserialize)]
pub struct MatchRequest {
    pub materials: Vec<String>,
    /// Optional: order each material's tests by how close they were run to
    /// this oxygen concentration (does not filter them out).
    pub oxygen_percent: Option<f64>,
}

/// `matches` is what web/src/scan.rs parses (SCAN_API.md); `results` mirrors it
/// under the backend's original key. Same array, both keys, so the site and
/// the documented API read the same thing.
#[derive(Debug, serde::Serialize)]
pub struct MatchResponse {
    pub matches: Vec<MaterialMatch>,
    pub results: Vec<MaterialMatch>,
}

/// SQL-only lookup against conditions + the reviewed synonym table.
/// No model call: exact and synonym matches must not depend on AI.
pub async fn materials_match(
    State(st): State<AppState>,
    Json(req): Json<MatchRequest>,
) -> ApiResult<Json<MatchResponse>> {
    let mut materials: Vec<String> = Vec::new();
    for m in &req.materials {
        let t = m.trim();
        if t.is_empty() {
            return Err(ApiError::from(Error::BadRequest(
                "materials must be non-empty strings".into(),
            )));
        }
        if t.len() > 100 {
            return Err(ApiError::from(Error::BadRequest(format!(
                "material \"{t}\" is too long (max 100 characters)"
            ))));
        }
        materials.push(t.to_string());
    }
    if materials.is_empty() {
        return Err(ApiError::from(Error::BadRequest(
            "materials array must contain at least one material".into(),
        )));
    }
    if materials.len() > MAX_MATERIALS {
        return Err(ApiError::from(Error::BadRequest(format!(
            "at most {MAX_MATERIALS} materials per request"
        ))));
    }
    if let Some(o2) = req.oxygen_percent {
        if !(0.0..=100.0).contains(&o2) {
            return Err(ApiError::from(Error::BadRequest(
                "oxygen_percent must be between 0 and 100".into(),
            )));
        }
    }

    let mut results = Vec::with_capacity(materials.len());
    for m in &materials {
        results.push(st.db.match_material(m, req.oxygen_percent).await?);
    }
    // `matches` and `results` are the same array (see MatchResponse).
    let matches = results.clone();
    Ok(Json(MatchResponse { matches, results }))
}
