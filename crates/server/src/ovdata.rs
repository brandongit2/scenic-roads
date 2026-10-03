//! Area overlays by view (docs/phase5.md "Areas"): their vector tiles (`layers/ov-*`, names
//! attached), their details by id and the parks' records (`ovdata/3-x-y`, pipeline::ovconv).

use crate::tiles::named_mvt_tile;
use crate::views::SectView;
use crate::S;
use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};

/// A z3 tile's overlay data: records (JSON) by id under a key ("harea", "indigenous", "special",
/// "parks").
pub struct OvView {
    sv: SectView,
    parks: OnceLock<Arc<Vec<Park>>>,
}

/// A park's record, with what the lookup by name near a point needs.
pub struct Park {
    pub norm: String,
    pub bbox: [f64; 4],
    pub area: f64,
    pub json: String,
}

impl OvView {
    pub fn new(sv: SectView) -> OvView {
        OvView { sv, parks: OnceLock::new() }
    }

    pub fn is_remote(&self) -> bool {
        self.sv.is_remote()
    }

    fn rec(&self, key: &str, k: usize) -> anyhow::Result<String> {
        let offs = self.sv.get(&format!("{key}.offs"))?;
        let offs: &[u32] = offs.cast();
        let recs = self.sv.get(&format!("{key}.recs"))?;
        let (a, b) = (offs[k] as usize, offs[k + 1] as usize);
        Ok(String::from_utf8_lossy(&recs.bytes()[a..b]).into_owned())
    }

    /// A record by id (None: not here).
    pub fn record(&self, key: &str, id: u64) -> anyhow::Result<Option<String>> {
        if !self.sv.has(&format!("{key}.ids")) {
            return Ok(None);
        }
        let ids = self.sv.get(&format!("{key}.ids"))?;
        let ids: &[u64] = ids.cast();
        match ids.binary_search(&id) {
            Ok(k) => Ok(Some(self.rec(key, k)?)),
            Err(_) => Ok(None),
        }
    }

    /// The parks owned here.
    pub fn parks(&self) -> anyhow::Result<Arc<Vec<Park>>> {
        if let Some(p) = self.parks.get() {
            return Ok(p.clone());
        }
        let mut out = Vec::new();
        if self.sv.has("parks.offs") {
            let n = self.sv.get("parks.offs")?.cast::<u32>().len().saturating_sub(1);
            for k in 0..n {
                let json = self.rec("parks", k)?;
                let v: serde_json::Value = serde_json::from_str(&json)?;
                let b = &v["bbox"];
                let bbox = [b[0].as_f64().unwrap_or(0.0), b[1].as_f64().unwrap_or(0.0), b[2].as_f64().unwrap_or(0.0), b[3].as_f64().unwrap_or(0.0)];
                let norm = crate::details::norm(v["name"].as_str().unwrap_or(""));
                out.push(Park { norm, bbox, area: v["area_km2"].as_f64().unwrap_or(0.0), json });
            }
        }
        Ok(self.parks.get_or_init(|| Arc::new(out)).clone())
    }
}

/// The area overlays' tiles: `/tiles/ov/{name}/{z}/{x}/{y}` from the catalog's `ov-{name}`.
pub async fn ov_tile(State(s): State<S>, Path((name, z, x, y)): Path<(String, u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    if !matches!(name.as_str(), "heritage-areas" | "indigenous" | "special" | "whs") {
        return StatusCode::NOT_FOUND.into_response();
    }
    named_mvt_tile(s, format!("ov-{name}"), crate::names_live::Rules::Areas, z, x, y, q, headers).await
}

#[derive(Deserialize)]
pub struct DetailQ {
    /// The z3 tile holding the details ("3/x/y", the feature's `own`).
    own: String,
}

/// An area's details: `/api/overlays/detail/{layer}/{id}?own=3/x/y`, with the descriptions.
pub async fn detail(State(s): State<S>, Path((layer, id)): Path<(String, u64)>, Query(q): Query<DetailQ>) -> Response {
    if !matches!(layer.as_str(), "harea" | "indigenous" | "special") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let Some(v) = s2.data.ovdata(&q.own)? else { return Ok(None) };
        v.record(&layer, id)
    })
    .await;
    match got {
        Ok(Ok(Some(rec))) => crate::details::json(&s.descriptions.apply(&rec)),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("overlays detail: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The parks of the z3 tiles around a point (a park is owned by its box's centre, within a tile of
/// any point in it), for `/api/park`.
pub fn parks_near(s: &crate::AppState, lon: f64, lat: f64) -> anyhow::Result<Vec<Arc<Vec<Park>>>> {
    let n = 8i64;
    let x = (((lon + 180.0) / 360.0 * n as f64).floor() as i64).clamp(0, n - 1);
    let sl = lat.clamp(-85.0, 85.0).to_radians().sin();
    let y = (((0.5 - ((1.0 + sl) / (1.0 - sl)).ln() / (4.0 * std::f64::consts::PI)) * n as f64).floor() as i64).clamp(0, n - 1);
    let mut out = Vec::new();
    for dx in -1..=1i64 {
        for dy in -1..=1i64 {
            let (tx, ty) = ((x + dx).rem_euclid(n), y + dy);
            if !(0..n).contains(&ty) {
                continue;
            }
            if let Some(v) = s.data.ovdata(&format!("3/{tx}/{ty}"))? {
                out.push(v.parks()?);
            }
        }
    }
    Ok(out)
}
