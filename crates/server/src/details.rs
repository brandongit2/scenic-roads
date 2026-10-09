//! The parks' popups: `GET /api/park?name=&lon=&lat=`, the protected area of that name around the
//! point (parks are drawn from the basemap's park layer, which has no ids), from the parks' records
//! in ovdata (the overlays job's); and what the details routes share.

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::S;

pub(crate) fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Details change with the user's descriptions: cached a minute.
pub(crate) fn json(body: &str) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=60")], body.to_string()).into_response()
}

#[derive(Deserialize)]
pub struct ParkQ {
    name: String,
    lon: f64,
    lat: f64,
}

/// The park of that name whose bounding box holds the point (the smallest, for nested ones), else
/// the nearest of that name within ~5 km, among the parks owned by the z3 tiles around the point.
pub async fn park(State(s): State<S>, Query(q): Query<ParkQ>) -> Response {
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let lists = crate::ovdata::parks_near(&s, q.lon, q.lat)?;
        let name = norm(&q.name);
        let named: Vec<&crate::ovdata::Park> = lists.iter().flat_map(|l| l.iter()).filter(|p| p.norm == name).collect();
        let pad = 0.002;
        let inside = named
            .iter()
            .filter(|p| q.lon >= p.bbox[0] - pad && q.lon <= p.bbox[2] + pad && q.lat >= p.bbox[1] - pad && q.lat <= p.bbox[3] + pad)
            .min_by(|a, b| a.area.total_cmp(&b.area));
        let pick = inside.or_else(|| {
            named
                .iter()
                .map(|p| {
                    let cx = q.lon.clamp(p.bbox[0], p.bbox[2]);
                    let cy = q.lat.clamp(p.bbox[1], p.bbox[3]);
                    (roadcore::dist_m(q.lon, q.lat, cx, cy), p)
                })
                .filter(|(m, _)| *m < 5000.0)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, p)| p)
        });
        Ok(pick.map(|p| p.json.clone()))
    })
    .await;
    match got {
        Ok(Ok(Some(j))) => json(&j),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("park: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
