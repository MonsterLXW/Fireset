use crate::{ApiError, ApiResult, AppState};
use ai_engine::extract_json_block;
use axum::{
    extract::{Multipart, Query, State},
    response::IntoResponse,
    Json as JsonResponse,
};
use serde::{Deserialize, Serialize};
use shared::Error;

pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Multipart framing overhead above the 5 MB image itself.
pub const MAX_SCAN_BODY_BYTES: usize = MAX_IMAGE_BYTES + 128 * 1024;

/// Accepted container formats, identified by magic bytes (never by the
/// client-declared content type alone).
fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[derive(Debug, Deserialize, Default)]
struct ScanModelJson {
    item: Option<String>,
    confidence: Option<f32>,
    parts: Option<Vec<ScanPartJson>>,
    needs_better_photo: Option<bool>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScanMaterialJson {
    material: Option<String>,
    share: Option<f64>,
    share_percent: Option<f64>,
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ScanPartJson {
    part: Option<String>,
    name: Option<String>,
    // A model that returns nested materials fills `materials`; one that
    // returns a flat part fills `material`/`share`. Both are accepted.
    materials: Option<Vec<ScanMaterialJson>>,
    material: Option<String>,
    share_percent: Option<f64>,
    share: Option<f64>,
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ScanModeQuery {
    mode: Option<String>,
}

// ---------- response (web/SCAN_API.md shape; see the field notes) ----------

#[derive(Debug, Clone, Serialize)]
pub struct ScanMaterialOut {
    pub material: String,
    /// Percent of the part believed to be this material; null when the share
    /// is an unsupported guess (a photo cannot prove a fiber blend).
    pub share: Option<i64>,
    /// Same value under the backend's original key.
    pub share_percent: Option<i64>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanPartOut {
    pub name: String,
    /// Same value under the backend's original key.
    pub part: String,
    pub materials: Vec<ScanMaterialOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResponse {
    pub item: Option<String>,
    pub confidence: f32,
    /// true when the photo could not be read (blurry/dark/no clear item) —
    /// the key the site reads.
    pub unclear: bool,
    /// Same meaning under the backend's original key.
    pub needs_better_photo: bool,
    /// Why the photo could not be read, else null.
    pub message: Option<String>,
    /// e.g. "vllm:qwen3.8-flash-next" — the provider:model that answered.
    pub model: String,
    /// A standing caveat: shares are visual estimates unless read from a label.
    pub note: String,
    pub parts: Vec<ScanPartOut>,
}

/// Brand name -> plain generic material (rule: no brand names as materials).
fn genericize_material(raw: &str) -> String {
    let m = raw.trim();
    match m.to_lowercase().as_str() {
        "teflon" => "PTFE".into(),
        "fluon" => "PTFE".into(),
        "kapton" => "polyimide".into(),
        "mylar" => "polyethylene terephthalate".into(),
        "plexiglass" | "plexiglas" | "perspex" | "acrylic" | "acrylic glass" => "PMMA".into(),
        "lycra" | "spandex" => "elastane".into(),
        "kevlar" | "nomex" | "technora" => "aramid".into(),
        "lexan" | "makrolon" => "polycarbonate".into(),
        "nylon" | "polyamide" => "nylon".into(),
        "poly" | "polythene" => "polyethylene".into(),
        "neoprene" => "polychloroprene".into(),
        "rayon" | "viscose" => "viscose (regenerated cellulose)".into(),
        _ => m.to_string(),
    }
}

const SYSTEM_PROMPT: &str = r#"You are the identification engine of "Burn Scan": someone photographs an item and we look up how its materials behaved in NASA microgravity fire tests.

RULES
1. Report only items and materials. If a person appears in the photo, describe only the item they wear or hold — never the person (no age, gender, appearance, or anything identifying).
2. Materials must be plain generic names: cotton, polyester, nylon, wool, aramid, viscose, polypropylene, polyethylene, polyethylene terephthalate, PMMA, polycarbonate, ABS, polyimide, PTFE, silicone, polyurethane foam, paper, cardboard, wood, aluminum, steel, copper, glass, etc. NEVER brand names (no "Teflon", "Kapton", "Mylar", "Velcro").
3. A photo cannot prove a fiber blend. Keep confidences honest on 0..1 (0.9+ only when clearly evidenced). If a part's material share is a guess you cannot support, set its share to null; give a number 1..100 only when you have real reason (e.g. a label).
4. needs_better_photo: true when the image is blurry, too dark, too close/cropped, or shows no clear item; then set item to null rather than guessing, and put a short reason in "message".
5. Output ONLY valid JSON, no prose:
{
  "item": "string or null",
  "confidence": 0.0,
  "needs_better_photo": false,
  "message": null,
  "parts": [
    { "name": "body", "materials": [
      { "material": "cotton", "share": 80, "confidence": 0.7 },
      { "material": "polyester", "share": 20, "confidence": 0.6 }
    ] }
  ]
}"#;

const MODE_ITEM_PROMPT: &str = "MODE item: identify the item and, per part, what it appears to be made of. Fiber blends are visual estimates — leave share null unless you can justify a number.";
const MODE_LABEL_PROMPT: &str = "MODE label: this photo is a fiber-content or materials label. Read the printed text and report exactly what it states (materials and listed percentages). Set item to the product if the label or photo names it. Confidence reflects reading certainty; if the text is unreadable, set needs_better_photo true and report nothing invented.";

/// Surface body-limit overruns as 413, anything else as 400. Walks the error
/// source chain because axum wraps the limit error a couple of layers deep.
fn map_multipart_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(err) = cur {
        let msg = err.to_string().to_lowercase();
        if msg.contains("too large") || msg.contains("limit") {
            return Error::PayloadTooLarge(format!(
                "request exceeds the {} MB multipart limit",
                MAX_SCAN_BODY_BYTES / (1024 * 1024)
            ))
            .into();
        }
        cur = err.source();
    }
    Error::BadRequest(format!("malformed multipart body: {e}")).into()
}

/// Raw-body overruns come back as BytesRejection; surface 413 vs 400.
fn map_body_err(e: axum::extract::rejection::BytesRejection) -> ApiError {
    let msg = e.to_string().to_lowercase();
    if msg.contains("too large") || msg.contains("larger") || msg.contains("limit") {
        Error::PayloadTooLarge(format!(
            "request exceeds the {} MB limit",
            MAX_SCAN_BODY_BYTES / (1024 * 1024)
        ))
        .into()
    } else {
        Error::BadRequest(format!("could not read the image body: {e}")).into()
    }
}

fn valid_mode(m: &str) -> bool {
    m == "item" || m == "label"
}

pub async fn ai_scan(
    State(st): State<AppState>,
    Query(q): Query<ScanModeQuery>,
    req: axum::extract::Request,
) -> ApiResult<impl IntoResponse> {
    use axum::body::Bytes;
    use axum::extract::FromRequest;

    let top_ct = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    // mode may arrive as a form field or a query param (the site passes it in
    // the query string); a form field wins if both are present.
    let mut mode = q.mode.unwrap_or_default().trim().to_lowercase();
    let mut image_bytes: Option<Vec<u8>> = None;
    let mut declared_ct: Option<String> = None;

    if top_ct.starts_with("multipart/") {
        let mut multipart = Multipart::from_request(req, &mut ())
            .await
            .map_err(|e| -> ApiError {
                // Only an invalid boundary can fail here (the extractor itself);
                // per-field errors still arrive as MultipartError below.
                ApiError::from(Error::BadRequest(format!("malformed multipart body: {e}")))
            })?;
        while let Some(field) = multipart.next_field().await.map_err(map_multipart_err)? {
            match field.name().unwrap_or("").to_lowercase().as_str() {
                "mode" => {
                    let text = field.text().await.map_err(map_multipart_err)?;
                    mode = text.trim().to_lowercase();
                }
                "image" => {
                    declared_ct = field.content_type().map(|s| s.to_lowercase());
                    let bytes = field.bytes().await.map_err(map_multipart_err)?;
                    if bytes.len() > MAX_IMAGE_BYTES {
                        return Err(Error::PayloadTooLarge(format!(
                            "image exceeds the {} MB limit",
                            MAX_IMAGE_BYTES / (1024 * 1024)
                        ))
                        .into());
                    }
                    if bytes.is_empty() {
                        return Err(Error::BadRequest("image field is empty".into()).into());
                    }
                    image_bytes = Some(bytes.to_vec()); // in memory only; never stored
                }
                _ => {} // ignore unknown fields
            }
        }
    } else {
        // Raw-body variant, for the dashboard's server-side proxy: the photo
        // bytes are the whole body with an image/... content type, and `mode`
        // comes from the query string only. Same limits and checks as the
        // multipart form below — this only changes how the bytes arrive.
        declared_ct = Some(
            top_ct
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        );
        let bytes = Bytes::from_request(req, &mut ())
            .await
            .map_err(map_body_err)?;
        if bytes.is_empty() {
            return Err(Error::BadRequest("image body is empty".into()).into());
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(Error::PayloadTooLarge(format!(
                "image exceeds the {} MB limit",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ))
            .into());
        }
        image_bytes = Some(bytes.to_vec()); // in memory only; never stored
    }

    if mode.is_empty() {
        mode = "item".to_string();
    }
    if !valid_mode(&mode) {
        return Err(Error::BadRequest(r#"mode must be "item" or "label""#.into()).into());
    }
    let bytes = image_bytes.ok_or_else(|| {
        ApiError::from(Error::BadRequest(
            "no image found: send a multipart form with an `image` file field, \
             or the photo bytes as the body with an image/... content type"
                .into(),
        ))
    })?;

    let media_type = sniff_media_type(&bytes).ok_or_else(|| {
        ApiError::from(Error::BadRequest(
            "image must be JPEG, PNG or WebP (could not recognize the file type)".into(),
        ))
    })?;
    if let Some(ct) = &declared_ct {
        if ct.starts_with("image/") && ct != media_type {
            return Err(Error::BadRequest(format!(
                "declared content type {ct} does not match file contents ({media_type})"
            ))
            .into());
        }
        if !ct.starts_with("image/") && ct != "application/octet-stream" {
            return Err(Error::BadRequest(format!("content type {ct} is not an image")).into());
        }
    }

    let resp = st
        .ai
        .complete_vision(
            Some(SYSTEM_PROMPT),
            &bytes,
            media_type,
            if mode == "label" { MODE_LABEL_PROMPT } else { MODE_ITEM_PROMPT },
            4096,
            0.1,
        )
        .await?;
    drop(bytes);

    let json_str = extract_json_block(&resp.text);
    let parsed: ScanModelJson = serde_json::from_str(json_str).map_err(|e| {
        Error::Pipeline(format!(
            "vision model returned unparseable JSON: {e}; raw: {}",
            resp.text.chars().take(300).collect::<String>()
        ))
    })?;

    let item = parsed.item.filter(|s| !s.trim().is_empty() && s.trim().ne("null"));

    // Group the model's parts by name; a material whose share can't be
    // supported stays with share = null (honest guess, not an invented split).
    let mut parts: Vec<ScanPartOut> = Vec::new();
    for p in parsed.parts.unwrap_or_default() {
        let name = p
            .name
            .or(p.part)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "item".to_string());
        // Nested materials win; a flat single-material part falls back.
        let raw_materials: Vec<ScanMaterialJson> = match p.materials.filter(|l| !l.is_empty()) {
            Some(list) => list,
            None => vec![ScanMaterialJson {
                material: p.material,
                share: p.share,
                share_percent: p.share_percent,
                confidence: p.confidence,
            }],
        };
        for m in raw_materials {
            let Some(material) = m.material.map(|x| genericize_material(&x)).filter(|x| !x.is_empty())
            else {
                continue;
            };
            let share = m.share.or(m.share_percent).and_then(|s| {
                if s > 0.0 && s <= 100.5 {
                    Some(s.round().clamp(1.0, 100.0) as i64)
                } else {
                    None
                }
            });
            let out = ScanMaterialOut {
                material,
                share,
                share_percent: share,
                confidence: m.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
            };
            match parts.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) {
                Some(existing) => existing.materials.push(out),
                None => parts.push(ScanPartOut {
                    part: name.clone(),
                    name: name.clone(),
                    materials: vec![out],
                }),
            }
        }
    }

    let mut unclear = parsed.needs_better_photo.unwrap_or(false);
    if item.is_none() && parts.is_empty() {
        unclear = true;
    }
    let message = if unclear {
        Some(
            parsed
                .message
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| "Could not read a clear item. Try a sharper, brighter, closer photo.".to_string()),
        )
    } else {
        parsed.message.filter(|m| !m.trim().is_empty())
    };

    let note = if mode == "label" {
        "Materials were read from the label text in the photo.".to_string()
    } else {
        "Fiber blend is a visual estimate; scan the label for exact content.".to_string()
    };

    Ok(JsonResponse(ScanResponse {
        item,
        confidence: parsed.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
        unclear,
        needs_better_photo: unclear,
        message,
        model: format!("vllm:{}", resp.model),
        note,
        parts,
    }))
}
