//! Tars (ustar) made and read whole in memory: the history's archived days (pipeline's
//! agent::histarchive, fcarchive) and a lake-depth tile's grids (pipeline's depth::lakes).

use anyhow::{ensure, Context, Result};

/// A tar (ustar) of `members`, in order: the whole stream.
pub fn build(members: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (name, b) in members {
        ensure!(name.len() <= 100, "{name}: name over 100 bytes");
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{:011o}", b.len()).as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&c| c as u32).sum();
        h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        h[154] = 0;
        h[155] = b' ';
        out.extend_from_slice(&h);
        out.extend_from_slice(b);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    Ok(out)
}

/// The members (name, bytes) of a tar made by `build`; an error when it is cut short or damaged.
pub fn list_tar(t: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut v = Vec::new();
    let mut at = 0;
    loop {
        ensure!(t.len() >= at + 512, "tar cut short");
        let h = &t[at..at + 512];
        if h.iter().all(|&c| c == 0) {
            ensure!(t.len() == at + 1024 && t[at + 512..].iter().all(|&c| c == 0), "tar end damaged");
            return Ok(v);
        }
        let end = h[..100].iter().position(|&c| c == 0).unwrap_or(100);
        let name = String::from_utf8(h[..end].to_vec())?;
        let size = usize::from_str_radix(std::str::from_utf8(&h[124..135])?, 8).context("size")?;
        let mut c = h.to_vec();
        c[148..156].copy_from_slice(b"        ");
        let sum: u32 = c.iter().map(|&x| x as u32).sum();
        ensure!(
            u32::from_str_radix(std::str::from_utf8(&h[148..154])?, 8).ok() == Some(sum),
            "{name}: header checksum"
        );
        let (a, b) = (at + 512, at + 512 + size);
        ensure!(t.len() >= b.div_ceil(512) * 512, "{name}: cut short");
        v.push((name, t[a..b].to_vec()));
        at = b.div_ceil(512) * 512;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_round_trips_and_detects_damage() {
        let m = vec![("a/x.jsonl".to_string(), vec![1u8; 700]), ("a/y.jsonl".to_string(), b"hi".to_vec())];
        let t = build(&m).unwrap();
        assert_eq!(t.len() % 512, 0);
        assert_eq!(list_tar(&t).unwrap(), m);
        assert!(list_tar(&t[..t.len() - 600]).is_err());
        let mut bad = t.clone();
        bad[3] ^= 1;
        assert!(list_tar(&bad).is_err());
    }
}
