//! The app's releases, kept and served by the lead (docs/coordinator.md §10): its Mac's own releases
//! (jobrt::appupdate::Releases, `<home>/app/releases/`, which its updater keeps as every Mac's does),
//! served to every Mac's updater and put live by a publish (coordwire::releases, the client's side).
//! A release goes in only checked as each Mac's updater will check it (the archive's SHA-256, then
//! every file unpacked by its own) and only over the live release it was checked against (its
//! `base`: publish.sh checked that the new commit contains it), under the releases' lock; a
//! rollback only from the two releases it was read as. The lead's alone: a member's port refuses
//! them (409). Every call carries the workers' key (the gate); none touches the NAS.

use super::http::Ctx;
use axum::body::Body;
use axum::extract::{Path as Url, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jobrt::appupdate::{archive_checked, unpack_checked, valid_version, Live, Manifest, Releases};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// The largest archive taken (a release is ~45 MB).
const UPLOAD_MAX: u64 = 2 << 30;
/// How long an upload may go without a byte.
const CHUNK_TIME: Duration = Duration::from_secs(120);

fn answer(code: StatusCode, v: serde_json::Value) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

fn refused(code: StatusCode, why: impl std::fmt::Display) -> Response {
    answer(code, serde_json::json!({ "error": why.to_string() }))
}

/// This coordinator's releases: the lead's alone, while its agent gave it them (`set_releases`).
fn releases(c: &Ctx) -> Result<Releases, Response> {
    let s = c.shared.lock().unwrap();
    if s.broker || s.viewer {
        return Err(refused(StatusCode::CONFLICT, "this Mac doesn't lead: the releases are the lead's"));
    }
    s.releases
        .clone()
        .map(Releases::at)
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, "this coordinator keeps no releases"))
}

/// Off the runtime: a call that reads or writes the disk.
async fn blocking(f: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e))
}

/// `GET /release/live`: the live release and the one before (none yet: both null).
pub(super) async fn live(State(c): State<Ctx>) -> Response {
    let r = match releases(&c) {
        Ok(r) => r,
        Err(e) => return e,
    };
    blocking(move || match r.live() {
        Ok(l) => answer(StatusCode::OK, serde_json::to_value(l).unwrap_or_default()),
        Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    })
    .await
}

/// `GET /release/<version>.tgz`: the archive of a release it holds live (or the one before).
pub(super) async fn archive(State(c): State<Ctx>, Url(file): Url<String>, h: HeaderMap) -> Response {
    let r = match releases(&c) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(v) = file.strip_suffix(".tgz").filter(|v| valid_version(v)).map(str::to_string) else {
        return refused(StatusCode::NOT_FOUND, format!("no {file}"));
    };
    let (r, held) = match tokio::task::spawn_blocking(move || {
        let held = r.live().is_ok_and(|l| {
            let (a, b) = l.versions();
            a == Some(v.as_str()) || b == Some(v.as_str())
        });
        (r, held.then_some(v))
    })
    .await
    {
        Ok(x) => x,
        Err(e) => return refused(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let Some(v) = held else { return refused(StatusCode::NOT_FOUND, format!("{file}: not a release the lead holds")) };
    super::http::send_file(
        &r.archive(&v),
        "application/octet-stream",
        None,
        h.get(header::RANGE).and_then(|v| v.to_str().ok()),
    )
    .await
}

/// A file being uploaded: removed unless it was kept.
struct Part(std::path::PathBuf, bool);
impl Drop for Part {
    fn drop(&mut self) {
        if !self.1 {
            std::fs::remove_file(&self.0).ok();
        }
    }
}

/// `PUT /release/upload/<version>`: a release's archive streamed into `incoming/` (its own name,
/// flushed to the disk), for its `install` to name: `{"upload": <its name>, "size"}`. One no
/// install took is removed by a later commit once an hour old (Releases::commit).
pub(super) async fn upload(State(c): State<Ctx>, Url(v): Url<String>, h: HeaderMap, body: Body) -> Response {
    let r = match releases(&c) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if h.get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > UPLOAD_MAX)
    {
        return refused(StatusCode::PAYLOAD_TOO_LARGE, "too big");
    }
    let dest = match r.incoming_file(&v) {
        Ok(p) => p,
        Err(e) => return refused(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    let mut part = Part(dest, false);
    let got: anyhow::Result<u64> = async {
        let mut f = tokio::fs::File::create(&part.0).await?;
        let mut stream = body.into_data_stream();
        let mut n = 0u64;
        loop {
            let next = tokio::time::timeout(CHUNK_TIME, futures_util::StreamExt::next(&mut stream))
                .await
                .map_err(|_| anyhow::anyhow!("no byte for {} s", CHUNK_TIME.as_secs()))?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| anyhow::anyhow!("{e}"))?;
            n += chunk.len() as u64;
            anyhow::ensure!(n <= UPLOAD_MAX, "too big");
            f.write_all(&chunk).await?;
        }
        f.flush().await?;
        f.sync_all().await?;
        Ok(n)
    }
    .await;
    match got {
        Ok(n) => {
            part.1 = true;
            let name = part.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            answer(StatusCode::OK, serde_json::json!({ "upload": name, "size": n }))
        }
        Err(e) => refused(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct Install {
    manifest: Manifest,
    /// The live version the publish checked its commit against (None: none live).
    base: Option<String>,
    /// Its archive's upload (`upload`'s name for it).
    upload: String,
}

/// `POST /release/install`: an uploaded release put live, checked as every Mac's updater checks
/// it (400 and the upload dropped, else), from `base` alone (409 with what is live, else); the
/// live one kept as the one before. 200: what is live now.
pub(super) async fn install(State(c): State<Ctx>, Json(i): Json<Install>) -> Response {
    let r = match releases(&c) {
        Ok(r) => r,
        Err(e) => return e,
    };
    blocking(move || install_now(&r, i)).await
}

fn install_now(r: &Releases, i: Install) -> Response {
    let m = i.manifest;
    let v = m.version.clone();
    if !valid_version(&v) || !i.upload.starts_with(&format!("{v}.")) || i.upload.contains('/') {
        return refused(StatusCode::BAD_REQUEST, format!("not {v}'s upload: {:?}", i.upload));
    }
    let up = r.incoming().join(&i.upload);
    if !up.is_file() {
        return refused(StatusCode::NOT_FOUND, format!("no upload {}", i.upload));
    }
    let up = Part(up, false);
    // (Checked unlocked: another publish's install or a rollback goes on meanwhile.)
    let check = up.0.with_extension("check");
    let checked = archive_checked(&up.0, &m).and_then(|()| unpack_checked(&up.0, &m, &check));
    std::fs::remove_dir_all(&check).ok();
    if let Err(e) = checked {
        return refused(StatusCode::BAD_REQUEST, format!("refused: {e:#}"));
    }
    let base = i.base;
    let next = |now: &Live| {
        let (live, _) = now.versions();
        (live == base.as_deref() && live != Some(v.as_str()))
            .then(|| Live { current: Some(m.clone()), previous: now.current.clone() })
    };
    match r.commit(next, &[(&v, &up.0)]) {
        Ok(Ok(next)) => {
            eprintln!("releases: {v} live (before it {})", next.versions().1.unwrap_or("none"));
            answer(StatusCode::OK, serde_json::json!({ "live": next }))
        }
        Ok(Err(now)) => answer(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": format!("the live release is {}, not {}: publish again from it",
                    now.versions().0.unwrap_or("none"), base.as_deref().unwrap_or("none")),
                "live": now,
            }),
        ),
        Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct Rollback {
    current: String,
    previous: String,
}

/// `POST /release/rollback`: the release before made live and the live one kept as the one
/// before, only from the two it was read as (409 with what is live, else). 200: what is live now.
pub(super) async fn rollback(State(c): State<Ctx>, Json(b): Json<Rollback>) -> Response {
    let r = match releases(&c) {
        Ok(r) => r,
        Err(e) => return e,
    };
    blocking(move || {
        let next = |now: &Live| {
            (now.versions() == (Some(b.current.as_str()), Some(b.previous.as_str())))
                .then(|| Live { current: now.previous.clone(), previous: now.current.clone() })
        };
        match r.commit(next, &[]) {
            Ok(Ok(next)) => {
                eprintln!("releases: rolled back to {}", b.previous);
                answer(StatusCode::OK, serde_json::json!({ "live": next }))
            }
            Ok(Err(now)) => answer(
                StatusCode::CONFLICT,
                serde_json::json!({
                    "error": format!("the lead's releases are {:?}, not {} and {} before it", now.versions(), b.current, b.previous),
                    "live": now,
                }),
            ),
            Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use crate::coord::client::Client;
    use crate::coord::{start_for_test, Coordinator};
    use coordwire::releases::{self as rel, Asked};
    use jobrt::appupdate::{Live, Manifest, Releases};
    use std::path::{Path, PathBuf};

    use jobrt::appupdate::sha256 as sha;

    /// A release as publish.sh makes it (lib.zsh's app_pack and app_manifest): its files packed in
    /// `<dir>/<v>.tgz`, and its manifest.
    fn release(dir: &Path, v: &str) -> (PathBuf, Manifest) {
        let files: [(&str, Vec<u8>); 2] =
            [("scenic", format!("#!/bin/sh\n# {v}\n").into_bytes()), ("web/index.html", v.into())];
        let src = dir.join(format!("{v}.src"));
        for (f, b) in &files {
            std::fs::create_dir_all(src.join(f).parent().unwrap()).unwrap();
            std::fs::write(src.join(f), b).unwrap();
        }
        let archive = dir.join(format!("{v}.tgz"));
        let st = store::spawn::status(
            store::spawn::command("/usr/bin/tar").arg("-czf").arg(&archive).arg("-C").arg(&src).args(["scenic", "web"]),
        )
        .unwrap();
        assert!(st.success());
        let m = Manifest {
            version: v.into(),
            sha256: files.iter().map(|(f, b)| (f.to_string(), sha(b))).collect(),
            archive_sha256: sha(&std::fs::read(&archive).unwrap()),
        };
        (archive, m)
    }

    /// A lead's coordinator keeping its releases in `releases`, and a client of it with the key.
    fn lead(dir: &Path, releases: &Path) -> (Coordinator, Client) {
        let (c, port) = start_for_test(dir, "m6", "");
        c.set_releases(releases.to_path_buf());
        let w = Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        (c, w)
    }

    fn versions(l: &Live) -> (Option<String>, Option<String>) {
        let (c, p) = l.versions();
        (c.map(str::to_string), p.map(str::to_string))
    }

    fn done(a: Asked) -> (Option<String>, Option<String>) {
        match a {
            Asked::Done(l) => versions(&l),
            Asked::Moved(l) => panic!("refused, live: {:?}", l.versions()),
        }
    }

    fn moved(a: Asked) -> (Option<String>, Option<String>) {
        match a {
            Asked::Moved(l) => versions(&l),
            Asked::Done(l) => panic!("went in: {:?}", l.versions()),
        }
    }

    const V0: &str = "20261001-0000-0000000";
    const V1: &str = "20261101-0000-aaaaaaa";
    const V2: &str = "20261102-0000-bbbbbbb";
    const V3: &str = "20261103-0000-ccccccc";

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    /// A publish's upload goes in only checked (its archive's SHA-256, then each file's: a refusal
    /// changes nothing and leaves no upload behind) and only over the live release it names; the
    /// lead holds it with the one before, serves both archives, and none older; nothing without the
    /// key.
    #[test]
    fn a_release_goes_in_checked_and_in_order() {
        let d = tempfile::tempdir().unwrap();
        let (store, src) = (d.path().join("releases"), d.path().join("src"));
        std::fs::create_dir_all(&src).unwrap();
        let (c, w) = lead(&d.path().join("coord"), &store);
        assert_eq!(rel::live(&w).unwrap(), Live::default());
        assert_eq!(jobrt::appupdate::Source::live(&w).unwrap(), None, "holds none yet");
        let (a1, m1) = release(&src, V1);
        assert_eq!(done(rel::push(&w, &a1, &m1, None).unwrap()), (s(V1), None));
        // Refused: an archive that isn't its manifest's; a file that doesn't match.
        let (a2, m2) = release(&src, V2);
        let mut bad = m2.clone();
        bad.archive_sha256 = sha(b"other");
        let e = rel::push(&w, &a2, &bad, Some(V1)).unwrap_err();
        assert!(format!("{e:#}").contains("its archive doesn't match"), "{e:#}");
        let mut bad = m2.clone();
        bad.sha256.insert("scenic".into(), sha(b"other"));
        let e = rel::push(&w, &a2, &bad, Some(V1)).unwrap_err();
        assert!(format!("{e:#}").contains("scenic doesn't match"), "{e:#}");
        assert!(std::fs::read_dir(store.join("incoming")).unwrap().next().is_none(), "no upload left");
        // The order: from the live release alone.
        assert_eq!(moved(rel::push(&w, &a2, &m2, None).unwrap()), (s(V1), None));
        assert_eq!(done(rel::push(&w, &a2, &m2, Some(V1)).unwrap()), (s(V2), s(V1)));
        let (a3, m3) = release(&src, V3);
        assert_eq!(moved(rel::push(&w, &a3, &m3, Some(V1)).unwrap()), (s(V2), s(V1)));
        assert_eq!(moved(rel::push(&w, &a2, &m2, Some(V2)).unwrap()), (s(V2), s(V1)), "already live");
        assert!(std::fs::read_dir(store.join("incoming")).unwrap().next().is_none(), "no upload left");
        // Both archives served as published; once V3 is in, V1's is gone.
        assert_eq!(w.get_bytes(&format!("/release/{V1}.tgz")).unwrap(), std::fs::read(&a1).unwrap());
        assert_eq!(w.get_bytes(&format!("/release/{V2}.tgz")).unwrap(), std::fs::read(&a2).unwrap());
        assert_eq!(done(rel::push(&w, &a3, &m3, Some(V2)).unwrap()), (s(V3), s(V2)));
        assert!(w.get_bytes(&format!("/release/{V1}.tgz")).is_err());
        assert!(!Releases::at(store.clone()).archive(V1).exists());
        assert_eq!(versions(&Releases::at(store.clone()).live().unwrap()), (s(V3), s(V2)));
        // Without the key: refused, every call.
        let nokey = Client::at(w.urls(), String::new(), "m1");
        assert_eq!(nokey.request("GET", rel::LIVE, None).unwrap().0, 403);
        assert_eq!(nokey.request("PUT", &format!("{}/{V3}", rel::UPLOAD), Some(b"x")).unwrap().0, 403);
        c.stop();
    }

    /// A rollback goes only from the two releases it was read as: the one before made live, the
    /// live one kept before it; asked again, refused; a publish after it goes from it.
    #[test]
    fn a_rollback_goes_from_the_two_it_was_read_as() {
        let d = tempfile::tempdir().unwrap();
        let (store, src) = (d.path().join("releases"), d.path().join("src"));
        std::fs::create_dir_all(&src).unwrap();
        let (c, w) = lead(&d.path().join("coord"), &store);
        let ((a1, m1), (a2, m2), (a3, m3)) = (release(&src, V1), release(&src, V2), release(&src, V3));
        done(rel::push(&w, &a1, &m1, None).unwrap());
        assert_eq!(moved(rel::rollback(&w, V1, V2).unwrap()), (s(V1), None), "none before it");
        done(rel::push(&w, &a2, &m2, Some(V1)).unwrap());
        assert_eq!(done(rel::rollback(&w, V2, V1).unwrap()), (s(V1), s(V2)));
        assert_eq!(moved(rel::rollback(&w, V2, V1).unwrap()), (s(V1), s(V2)));
        assert_eq!(done(rel::push(&w, &a3, &m3, Some(V1)).unwrap()), (s(V3), s(V1)));
        c.stop();
    }

    /// A member's home, its app on V0.
    fn member(home: &Path) -> PathBuf {
        std::fs::create_dir_all(home.join("app").join(V0)).unwrap();
        std::os::unix::fs::symlink(V0, home.join("app/current")).unwrap();
        home.to_path_buf()
    }

    fn current(home: &Path) -> String {
        std::fs::read_link(home.join("app/current")).unwrap().to_string_lossy().into_owned()
    }

    /// A member's app follows the lead: the new release fetched, checked and made current, held
    /// with the one before; the lead stopped, the update waits.
    #[test]
    fn a_member_updates_from_the_lead_and_waits_while_it_is_down() {
        let d = tempfile::tempdir().unwrap();
        let (store, src) = (d.path().join("releases"), d.path().join("src"));
        std::fs::create_dir_all(&src).unwrap();
        let (c, w) = lead(&d.path().join("coord"), &store);
        let home = member(&d.path().join("m1"));
        assert_eq!(jobrt::appupdate::check(&home, &w, V0).unwrap(), None, "none yet");
        let ((a1, m1), (a2, m2)) = (release(&src, V1), release(&src, V2));
        done(rel::push(&w, &a1, &m1, None).unwrap());
        done(rel::push(&w, &a2, &m2, Some(V1)).unwrap());
        assert_eq!(jobrt::appupdate::check(&home, &w, V0).unwrap(), s(V2));
        assert_eq!(current(&home), V2);
        assert_eq!(std::fs::read(home.join("app").join(V2).join("web/index.html")).unwrap(), V2.as_bytes());
        assert_eq!(versions(&Releases::of_home(&home).live().unwrap()), (s(V2), s(V1)));
        assert_eq!(std::fs::read(Releases::of_home(&home).archive(V1)).unwrap(), std::fs::read(&a1).unwrap());
        // The lead down: an error, nothing changed.
        let (a3, m3) = release(&src, V3);
        done(rel::push(&w, &a3, &m3, Some(V2)).unwrap());
        c.stop();
        assert!(jobrt::appupdate::check(&home, &w, V2).is_err());
        assert_eq!(current(&home), V2);
        assert_eq!(versions(&Releases::of_home(&home).live().unwrap()), (s(V2), s(V1)));
    }

    /// The lead moved (the owner's edit of the declaration): the new lead serves the releases its
    /// Mac followed as a member, the live one and the one before, so the Macs update from it, a
    /// rollback goes from them and a publish follows them.
    #[test]
    fn a_new_lead_serves_the_releases_it_followed() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let (old, w) = lead(&d.path().join("coord-a"), &d.path().join("releases-a"));
        let ((a1, m1), (a2, m2), (a3, m3)) = (release(&src, V1), release(&src, V2), release(&src, V3));
        done(rel::push(&w, &a1, &m1, None).unwrap());
        done(rel::push(&w, &a2, &m2, Some(V1)).unwrap());
        let b = member(&d.path().join("b"));
        assert_eq!(jobrt::appupdate::check(&b, &w, V0).unwrap(), s(V2));
        old.stop();
        // B leads: its coordinator serves its own Mac's releases.
        let (new, w) = lead(&d.path().join("b/agent/coord"), &b.join(jobrt::appupdate::RELEASES));
        assert_eq!(versions(&rel::live(&w).unwrap()), (s(V2), s(V1)));
        let m = member(&d.path().join("m"));
        assert_eq!(jobrt::appupdate::check(&m, &w, V0).unwrap(), s(V2));
        assert_eq!(done(rel::rollback(&w, V2, V1).unwrap()), (s(V1), s(V2)));
        assert_eq!(jobrt::appupdate::check(&m, &w, V2).unwrap(), s(V1));
        assert_eq!(done(rel::push(&w, &a3, &m3, Some(V1)).unwrap()), (s(V3), s(V1)));
        // (B's own updater, asking its own coordinator, holds what it serves already: only its app
        // changes.)
        assert_eq!(jobrt::appupdate::check(&b, &w, V2).unwrap(), s(V3));
        assert_eq!(versions(&Releases::of_home(&b).live().unwrap()), (s(V3), s(V1)));
        new.stop();
    }
}
