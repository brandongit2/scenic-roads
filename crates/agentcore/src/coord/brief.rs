//! The records' brief by HTTP (docs/coordination.md step 8d, task #1037): `GET /pool/brief` on the
//! lead's coordinator answers with the brief of the records its agent last saved whole (on its own
//! disk and the NAS's one path: the bytes of `state/build/brief.json`), from memory. Its `ETag` is
//! the records' (epoch, seq); a request whose `If-None-Match` names it is answered 304, with no
//! body. The body is gzipped once per save (`Served::of`, ~6.5 MB to ~1.6 MB on 11 Nov), sent as it
//! is to a client that takes gzip (every agent's: ureq's), unpacked for one that doesn't. No brief
//! saved yet in this process (a lead before its first save, a member's broker): 503, and the
//! asker reads the NAS's or waits (crate::agent::brief). Nothing here touches the NAS (§2.3).

use crate::pool::records::{Brief, Records};
use anyhow::Result;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::io::{Read, Write};
use std::sync::Arc;

/// The route.
pub const PATH: &str = "/pool/brief";

/// A brief as a lead serves it and its agent reads it: its records' (epoch, seq), the brief parsed,
/// and its bytes as the NAS's file has them.
#[derive(Clone, Debug)]
pub struct Held {
    pub mark: (u64, u64),
    pub brief: Arc<Brief>,
    pub json: Arc<Vec<u8>>,
}

impl Held {
    /// The brief of records `r`, as `Records::save` writes it to the NAS.
    pub fn of(r: &Records) -> Result<Held> {
        let brief = Brief { epoch: r.epoch, seq: r.seq, manifest: r.manifest.clone(), keys: r.keys.clone() };
        let json = serde_json::to_vec(&brief)?;
        Ok(Held { mark: (r.epoch, r.seq), brief: Arc::new(brief), json: Arc::new(json) })
    }

    /// A brief's bytes as served (`json`): parsed, its mark its own.
    pub fn parse(json: Vec<u8>) -> Result<Held> {
        let brief: Brief = serde_json::from_slice(&json)?;
        Ok(Held { mark: (brief.epoch, brief.seq), brief: Arc::new(brief), json: Arc::new(json) })
    }

    /// Its `ETag`: `"<epoch>-<seq>"`.
    pub fn tag(&self) -> String {
        tag(self.mark)
    }
}

/// The `ETag` of a brief of records (epoch, seq).
pub fn tag((epoch, seq): (u64, u64)) -> String {
    format!("\"{epoch}-{seq}\"")
}

/// The brief a lead serves: held, and gzipped.
#[derive(Debug)]
pub struct Served {
    pub held: Held,
    gz: Vec<u8>,
}

impl Served {
    /// Records `r`'s brief, gzipped once here (off the coordinator's lock: its agent's step).
    pub fn of(r: &Records) -> Result<Served> {
        let held = Held::of(r)?;
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&held.json)?;
        Ok(Served { gz: z.finish()?, held })
    }

    /// Its bytes on the wire.
    pub fn gz_len(&self) -> usize {
        self.gz.len()
    }
}

/// Whether `If-None-Match` in `h` names `tag` (or `*`).
fn unchanged(h: &HeaderMap, tag: &str) -> bool {
    h.get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == tag || t == "*")
}

/// Whether the request's `Accept-Encoding` in `h` takes gzip.
fn takes_gzip(h: &HeaderMap) -> bool {
    h.get_all(header::ACCEPT_ENCODING).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).any(|e| {
        let mut parts = e.split(';');
        let name = parts.next().unwrap_or("").trim();
        let q0 = parts.any(|p| p.trim().strip_prefix("q=").and_then(|q| q.trim().parse::<f32>().ok()) == Some(0.0));
        (name.eq_ignore_ascii_case("gzip") || name == "*") && !q0
    })
}

/// The answer to a request with headers `h`, `served` the brief held now (None: none yet).
pub fn answer(served: Option<&Served>, h: &HeaderMap) -> Response {
    let Some(s) = served else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({ "error": "no records' brief saved yet by this coordinator's agent" })),
        )
            .into_response();
    };
    let tag = s.held.tag();
    let Ok(etag) = HeaderValue::from_str(&tag) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let common = [(header::ETAG, etag), (header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))];
    if unchanged(h, &tag) {
        return (StatusCode::NOT_MODIFIED, common).into_response();
    }
    let json = HeaderValue::from_static("application/json");
    let vary = HeaderValue::from_static("accept-encoding");
    if takes_gzip(h) {
        let gz = HeaderValue::from_static("gzip");
        return (
            common,
            [(header::CONTENT_TYPE, json), (header::CONTENT_ENCODING, gz), (header::VARY, vary)],
            s.gz.clone(),
        )
            .into_response();
    }
    // (A client that takes no gzip, a person's curl: unpacked, as the NAS's file.)
    let mut b = Vec::with_capacity(s.held.json.len());
    if flate2::read::GzDecoder::new(&s.gz[..]).read_to_end(&mut b).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    (common, [(header::CONTENT_TYPE, json), (header::VARY, vary)], b).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(seq: u64) -> Records {
        let mut r = Records { epoch: 849, seq, ..Default::default() };
        r.manifest.insert("base/6-1-1".into(), format!("base/6-1-1.{seq:016}.base"));
        r.pending.insert("not/in-the-brief".into(), "aa".into());
        r
    }

    async fn body(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    /// The brief served is the NAS's file's bytes (`Records::save`'s), tagged by its records'
    /// (epoch, seq): gzipped to a client that takes it, unpacked else; 304 with no body to one
    /// that names its tag; 503 with none held.
    #[tokio::test]
    async fn the_brief_is_answered_by_its_tag() {
        let r = records(4);
        use crate::pool::nas::Nas;
        let mem = crate::pool::nas::Mem::default();
        let mut saved = r.clone();
        saved.seq -= 1;
        saved.save(&mem, &|_| Ok(())).unwrap();
        let on_nas = mem.read(crate::pool::records::ONE_BRIEF).unwrap().unwrap();
        let s = Served::of(&saved).unwrap();
        assert_eq!(*s.held.json, on_nas, "the NAS's bytes");
        assert!(s.gz_len() < on_nas.len());
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        let a = answer(Some(&s), &h);
        assert_eq!(a.status(), StatusCode::OK);
        assert_eq!(a.headers()[header::ETAG], "\"849-4\"");
        assert_eq!(a.headers()[header::CONTENT_ENCODING], "gzip");
        let mut un = Vec::new();
        flate2::read::GzDecoder::new(&body(a).await[..]).read_to_end(&mut un).unwrap();
        assert_eq!(un, on_nas);
        let plain = answer(Some(&s), &HeaderMap::new());
        assert!(plain.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(body(plain).await, on_nas);
        let mut refused = HeaderMap::new();
        refused.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip;q=0"));
        assert!(answer(Some(&s), &refused).headers().get(header::CONTENT_ENCODING).is_none());
        // Unchanged: 304, nothing more.
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"849-3\", \"849-4\""));
        let same = answer(Some(&s), &h);
        assert_eq!(same.status(), StatusCode::NOT_MODIFIED);
        assert!(body(same).await.is_empty());
        // An older tag: the whole brief.
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"849-3\""));
        assert_eq!(answer(Some(&s), &h).status(), StatusCode::OK);
        assert_eq!(answer(None, &h).status(), StatusCode::SERVICE_UNAVAILABLE);
        let back = Held::parse(on_nas).unwrap();
        assert_eq!((back.mark, back.brief.manifest.clone()), ((849, 4), s.held.brief.manifest.clone()));
    }
}
