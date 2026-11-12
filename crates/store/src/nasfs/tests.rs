use super::*;

fn root() -> (tempfile::TempDir, NasRoot) {
    let d = tempfile::tempdir().unwrap();
    let r = NasRoot::local_for_test(d.path());
    (d, r)
}

/// Every name and byte in a folder, recursively ('/'-joined names).
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    fn walk(base: &Path, d: &Path, v: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                v.push((rel + "/", Vec::new()));
                walk(base, &p, v);
            } else {
                v.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(dir, dir, &mut v);
    v.sort();
    v
}

/// The layer's writes leave the same names and bytes as the raw calls they replace: whole::write,
/// std::fs::write, whole::copy both ways, create-new, a streamed create.
#[test]
fn writes_are_byte_identical_to_the_raw_calls() {
    let (a, r) = root();
    let b = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("src.bin");
    std::fs::write(&src, (0..5000u32).map(|i| (i * 7) as u8).collect::<Vec<_>>()).unwrap();
    let bytes: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();

    std::fs::create_dir_all(b.path().join("state/build")).unwrap();
    r.join("state/build").create_dir_all().unwrap();
    // whole::write, twice over one name.
    crate::whole::write(&b.path().join("state/build/manifest.json"), b"{}").unwrap();
    crate::whole::write(&b.path().join("state/build/manifest.json"), &bytes).unwrap();
    r.join("state/build/manifest.json").write_whole(b"{}").unwrap();
    r.join("state").join("build/manifest.json").write_whole(&bytes).unwrap();
    // std::fs::write in place.
    std::fs::write(b.path().join("state/plain.txt"), b"plain").unwrap();
    r.join("state/plain.txt").write(b"plain").unwrap();
    // Copies in and out.
    crate::whole::copy(&src, &b.path().join("state/in.bin")).unwrap();
    r.join("state/in.bin").copy_from_local(&src).unwrap();
    crate::whole::copy(&b.path().join("state/in.bin"), &local.path().join("out-b.bin")).unwrap();
    r.join("state/in.bin").copy_to_local(&local.path().join("out-a.bin")).unwrap();
    assert_eq!(
        std::fs::read(local.path().join("out-a.bin")).unwrap(),
        std::fs::read(local.path().join("out-b.bin")).unwrap()
    );
    // Create-new into a folder not made yet, and a streamed create.
    std::fs::create_dir_all(b.path().join("state/build/terms")).unwrap();
    std::fs::write(b.path().join("state/build/terms/1.json"), b"one").unwrap();
    assert!(matches!(r.join("state/build/terms/1.json").create_new(b"one").unwrap(), Created::Made));
    std::fs::write(b.path().join("state/log.txt"), b"ab").unwrap();
    {
        let mut f = r.join("state/log.txt").create().unwrap();
        f.write_all(b"a").unwrap();
        f.write_all(b"b").unwrap();
    }
    assert_eq!(tree(a.path()), tree(b.path()));
}

/// The temporary name is whole::tmp_name's, so GC and other Macs still know one mid-flight.
#[test]
fn the_temporary_name_is_unchanged() {
    let (d, r) = root();
    let p = r.join("state/build/manifest.json");
    assert_eq!(p.tmp_name().unchecked_path(), crate::whole::tmp_name(&d.path().join("state/build/manifest.json")));
    assert!(crate::whole::is_tmp(&p.tmp_name().unchecked_path()));
}

#[test]
fn paths_join_and_never_leave_the_root() {
    let (d, r) = root();
    let p = r.join("sources/").join("./buildings//2026/x.f32");
    assert_eq!(p.rel(), "sources/buildings/2026/x.f32");
    assert_eq!(p.unchecked_path(), d.path().join("sources/buildings/2026/x.f32"));
    assert_eq!(p.file_name(), Some("x.f32"));
    assert_eq!(p.extension(), Some("f32"));
    assert_eq!(p.with_extension("tmp").rel(), "sources/buildings/2026/x.tmp");
    assert_eq!(p.parent().unwrap().rel(), "sources/buildings/2026");
    assert_eq!(r.join("a").parent().unwrap().rel(), "");
    assert!(r.path().parent().is_none());
    assert_eq!(p.rel_under(&r).as_deref(), Some("sources/buildings/2026/x.f32"));
    assert_eq!(
        NasPath::local_for_test(d.path().join("sources/buildings/2026/x.f32")),
        p,
        "the same file, from another root"
    );
    assert_eq!(format!("{p:?}"), "nas:sources/buildings/2026/x.f32");
    assert_eq!(r.path().unchecked_path(), d.path());
    assert!(std::panic::catch_unwind(|| r.join("a/../b")).is_err());
    assert!(std::panic::catch_unwind(|| r.join("/etc")).is_err());
}

#[test]
#[should_panic(expected = "the real NAS")]
fn a_tests_root_is_never_the_nas() {
    NasRoot::local_for_test("/Volumes/personal/projects/scenic-roads");
}

/// Reads, stats, listings and writes count as nasio says.
#[test]
fn every_operation_is_counted() {
    let (_d, r) = root();
    let at = nasio::own();
    let d = |c: Count| nasio::own()[c as usize] - at[c as usize];
    r.join("x").create_dir_all().unwrap();
    r.join("x/a.json").write_whole(b"12345").unwrap();
    r.join("x/b.json").write(b"123").unwrap();
    assert!(d(Count::WroteBytes) >= 8 && d(Count::Creates) >= 2);
    assert_eq!(r.join("x/a.json").read().unwrap(), b"12345");
    assert!(d(Count::ReadBytes) >= 5 && d(Count::Files) >= 1);
    let stats = d(Count::Stats);
    assert!(r.join("x/a.json").exists());
    assert!(d(Count::Stats) > stats);
    assert_eq!(r.join("x").list().unwrap(), ["a.json", "b.json"]);
    assert!(d(Count::Listed) >= 2);
    r.join("x/b.json").remove_file().unwrap();
    assert!(d(Count::Deletes) >= 1);
}

/// A file copied whole while this Mac's page cache holds it (one just written) counts as read
/// here (`Local`), not from the NAS, task #939; the copy's file is counted as opened all the same.
#[cfg(unix)]
#[test]
fn a_copy_the_page_cache_answers_is_read_here() {
    let (d, r) = root();
    let n = 3 << 20;
    r.join("p.pack").write(vec![5u8; n]).unwrap();
    std::fs::read(d.path().join("p.pack")).unwrap();
    let to = d.path().join("copies");
    std::fs::create_dir_all(&to).unwrap();
    let at = nasio::own();
    let grew = |c: Count| nasio::own()[c as usize] - at[c as usize];
    assert_eq!(r.join("p.pack").copy_data_to_local(&to.join("a")).unwrap(), n as u64);
    assert_eq!(r.join("p.pack").copy_to_local(&to.join("b")).unwrap(), n as u64);
    assert!(grew(Count::Local) >= 2 * n as u64, "{} local", grew(Count::Local));
    assert!(grew(Count::Files) >= 2);
}

/// A read the share fails for a moment is tried again; one that is missing fails at once.
#[test]
fn reads_ride_out_a_transient_error_and_missing_is_missing() {
    crate::retry::fast();
    let (_d, r) = root();
    let p = r.join("f");
    let n = std::cell::Cell::new(0);
    let got = p
        .retry("read", || {
            n.set(n.get() + 1);
            if n.get() < 3 {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(7)
            }
        })
        .unwrap();
    assert_eq!((got, n.get()), (7, 3));
    assert_eq!(p.read().unwrap_err().kind(), io::ErrorKind::NotFound);
    assert!(!p.exists());
    assert_eq!(p.try_exists().unwrap(), false);
    // The server's root: the short schedule.
    assert_eq!(r.short_retry().waits().len(), crate::retry::SHORT_WAITS.len());
    assert_eq!(r.waits().len(), crate::retry::WAITS.len());
    // The agent's loop's root: one try, its error as it came; its copies share what was made.
    let once = r.no_retry();
    let n = std::cell::Cell::new(0);
    let e = once
        .join("x")
        .retry("read", || -> io::Result<()> {
            n.set(n.get() + 1);
            Err(io::Error::from_raw_os_error(libc::EIO))
        })
        .unwrap_err();
    assert_eq!((n.get(), e.raw_os_error()), (1, Some(libc::EIO)));
    once.join("made/a").write_whole_made(b"a").unwrap();
    assert!(r.join("made/b").parent_remembered() && r.long_retry().join("made/c").parent_remembered());
}

/// A read past the cache reads what a write left.
#[test]
fn a_fresh_read_reads_what_was_written() {
    let (_d, r) = root();
    let p = r.join("state/pool/x.json");
    assert_eq!(p.read_fresh().unwrap_err().kind(), io::ErrorKind::NotFound);
    p.write_whole_made(b"{\"beat\":1}").unwrap();
    assert_eq!(p.read_fresh().unwrap(), b"{\"beat\":1}");
}

/// A folder's make that says it exists (SMB's stale stat after another Mac made it) isn't an
/// error: the write that follows says whether it's there. A folder made is made once a process, and
/// made again after a write into it fails (it went).
#[test]
fn a_folder_said_to_exist_is_taken_and_made_once() {
    let (d, r) = root();
    std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
    std::fs::write(d.path().join("state/pool/members"), b"x").unwrap();
    let e = r.join("state/pool/members/m-1.json").write_whole_made(b"a").unwrap_err();
    assert!(!format!("{e:#}").starts_with("make "), "{e:#}");
    assert!(r.join("state/pool/members/m-2.json").create_new(b"a").is_err());
    std::fs::remove_file(d.path().join("state/pool/members")).unwrap();
    r.join("state/pool/members/m-1.json").write_whole_made(b"a").unwrap();
    assert!(r.join("state/pool/members/m-1.json").parent_remembered());
    std::fs::remove_dir_all(d.path().join("state/pool/members")).unwrap();
    r.join("state/pool/members/m-1.json").write_whole_made(b"b").unwrap();
    assert_eq!(r.join("state/pool/members/m-1.json").read().unwrap(), b"b");
    std::fs::remove_dir_all(d.path().join("state/pool/members")).unwrap();
    assert!(matches!(r.join("state/pool/members/m-3.json").create_new(b"c").unwrap(), Created::Made));
    assert!(matches!(r.join("state/pool/members/m-3.json").create_new(b"d").unwrap(), Created::There));
    assert_eq!(r.join("state/pool/members/m-3.json").read().unwrap(), b"c");
}

#[test]
fn a_listing_leaves_temporary_files_out_and_a_missing_folder_is_empty() {
    let (_d, r) = root();
    r.join("m").create_dir_all().unwrap();
    r.join("m/m-1.json").write(b"a").unwrap();
    r.join("m/m-2.json.mac.7.tmp").write(b"x").unwrap();
    assert_eq!(r.join("m").list().unwrap(), ["m-1.json"]);
    assert!(r.join("none").list().unwrap().is_empty());
    assert_eq!(r.join("none").read_dir().unwrap_err().kind(), io::ErrorKind::NotFound);
    let e = r.join("m").read_dir().unwrap();
    assert_eq!(e.len(), 2);
    assert!(e.iter().all(|e| e.is_file() && e.path.parent().unwrap() == r.join("m")));
}

/// Task #1004: an entry's stat is the link's own, never what it leads to (a link to its own
/// folder, a link to a file).
#[test]
fn an_entrys_stat_never_follows_a_link() {
    let (d, r) = root();
    r.join("m").create_dir_all().unwrap();
    r.join("m/a.json").write(b"a").unwrap();
    std::os::unix::fs::symlink(d.path().join("m"), d.path().join("m/m")).unwrap();
    std::os::unix::fs::symlink(d.path().join("m/a.json"), d.path().join("m/b.json")).unwrap();
    let mut e = r.join("m").read_dir().unwrap();
    e.sort_by(|a, b| a.name.cmp(&b.name));
    let kinds: Vec<(&str, bool, bool)> = e
        .iter()
        .map(|e| {
            let m = e.symlink_metadata().unwrap();
            (e.name.as_str(), m.is_dir(), m.file_type().is_symlink())
        })
        .collect();
    assert_eq!(kinds, [("a.json", false, false), ("b.json", false, true), ("m", false, true)]);
}

/// Task #990: an upload's bytes count as each write goes, not once it lands, so a long upload's job
/// is seen moving by its beats; the file counts made as it lands. A file written through `NasFile`,
/// a whole copy to the NAS (each read told), a NAS-to-NAS copy, a content-named upload.
#[test]
fn an_uploads_bytes_count_as_they_go() {
    let (_d, r) = root();
    let at = nasio::own();
    let d = |c: Count| nasio::own()[c as usize] - at[c as usize];
    r.join("x").create_dir_all().unwrap();
    let mut f = r.join("x/streamed.bin").create().unwrap();
    f.write_all(&[7u8; 3000]).unwrap();
    let creates = d(Count::Creates);
    assert!(d(Count::WroteBytes) >= 3000, "counted as written, before the file is done");
    drop(f);
    assert!(d(Count::Creates) > creates, "the file counted made as it closes");
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("src.bin");
    std::fs::write(&src, vec![1u8; 300_000]).unwrap();
    let mut told = Vec::new();
    crate::whole::copy_told(&src, &local.path().join("dst.bin"), &mut |k| told.push(k)).unwrap();
    assert_eq!(told.iter().sum::<u64>(), 300_000, "every byte told");
    assert!(told.len() > 1, "told a read at a time, not once");
    let wrote = d(Count::WroteBytes);
    r.join("x/copied.bin").copy_from_local(&src).unwrap();
    r.join("x/copied.bin").copy_data_to(&r.join("x/again.bin")).unwrap();
    assert!(d(Count::WroteBytes) >= wrote + 600_000);
    let wrote = d(Count::WroteBytes);
    let name = crate::naming::write_atomic(&r, "x/named", "bin", crate::naming::Source::File(&src)).unwrap();
    assert!(r.join(&name).exists());
    assert!(d(Count::WroteBytes) >= wrote + 300_000, "a content-named upload's bytes, as it writes them");
}

/// Task #1142: a NAS file's close says its error (where the Mac's SMB client gives a write's last
/// one): `NasFile::close`, an in-place `write`, a create-new; the file still counted made.
#[test]
fn a_close_error_is_the_writes() {
    use crate::sys::close_fault::fail;
    let (d, r) = root();
    let at = nasio::own();
    let f = fail(d.path(), libc::ETIMEDOUT, 3);
    let mut w = r.join("closed.bin").create().unwrap();
    w.write_all(b"abc").unwrap();
    w.sync_all().unwrap();
    assert_eq!(w.close().unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
    assert!(nasio::own()[Count::Creates as usize] > at[Count::Creates as usize], "counted made");
    assert_eq!(r.join("in-place.json").write(b"x").unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
    assert!(matches!(r.join("new.json").create_new(b"y").unwrap(), Created::Unwritten(_)));
    assert_eq!(f.failed(), 3);
    drop(f);
    let mut w = r.join("closed.bin").create().unwrap();
    w.write_all(b"abc").unwrap();
    w.close().unwrap();
    assert_eq!(std::fs::read(d.path().join("closed.bin")).unwrap(), b"abc");
}

/// Task #1141: a call the share never answers fails at its deadline (a read, a stat, a write),
/// left to the kernel; while that write is held a later write of its path is refused at once,
/// never made, and once the share answers the path is free again and the last write lands.
#[test]
fn a_call_never_answered_is_given_up_and_a_held_write_holds_its_path() {
    use crate::deadline::{fake, past_deadline, refused};
    let (d, r) = root();
    let cap = crate::deadline::cap_for_test(crate::deadline::CAP_DEFAULT);
    let p = r.join("f.json");
    p.write_whole(b"1").unwrap();
    let hung = fake::hang(&d.path().join("f.json")).deadline(Duration::from_millis(200));
    let io = |e: anyhow::Error| e.downcast::<io::Error>().unwrap();
    assert!(past_deadline(&p.read().unwrap_err()));
    assert!(past_deadline(&p.metadata().unwrap_err()));
    assert!(past_deadline(&io(p.write_whole(b"2").unwrap_err())));
    let t = Instant::now();
    let e = io(p.write_whole(b"3").unwrap_err());
    assert!(refused(&e) && e.to_string().contains("still in the kernel"), "{e}");
    crate::testkit::ended_before(t, Duration::from_millis(200), "refused at once");
    assert_eq!(cap.held(), 3);
    // (Another file goes on meanwhile.)
    r.join("g.json").write_whole(b"g").unwrap();
    drop(hung);
    crate::testkit::eventually("the held calls end", || cap.held() == 0);
    p.write_whole(b"4").unwrap();
    assert_eq!(p.read().unwrap(), b"4");
}
