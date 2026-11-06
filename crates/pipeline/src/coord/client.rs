//! A worker's side of the coordinator. Its client is coordwire's (coordwire::client: the contact on
//! the NAS, the rounds, the kept connections, the files' transfers), re-exported here; the lease
//! calls on it are here (`LeaseCalls`: they carry the coordinator's own types), with what they
//! answer and `probe`.

pub use coordwire::client::{Answered, Client, Reply};

use super::{Ask, Done, Fail, Grant};
use anyhow::{bail, Result};
use std::time::Duration;

/// Work refused to this worker as it is (an agent on an older app than the lead's, or the build
/// paused): why, in words for its status, and the build's pause, when that's why.
#[derive(Debug)]
pub struct Refused {
    pub why: String,
    pub pause: Option<crate::control::Pause>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for Refused {}

/// Whether a coordinator (or a broker) answers at any of `urls`, trying each in turn with a short
/// timeout (crate::pool::reachable::PROBE_TIMEOUT_S): any HTTP answer counts, whatever its status
/// (it needs no token); else the last failure, its cause read from it.
pub fn probe(urls: &[String]) -> std::result::Result<(), (crate::pool::reachable::Cause, String)> {
    use crate::pool::reachable::{cause_of, Cause, PROBE_TIMEOUT_S};
    let t = Duration::from_secs(PROBE_TIMEOUT_S);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(t))
        .timeout_global(Some(t))
        .http_status_as_error(false)
        .build()
        .into();
    let mut last = (Cause::Other, "no address".to_string());
    let _w = store::waits::net();
    for url in urls {
        match agent.get(&format!("{url}/")).call() {
            Ok(_) => return Ok(()),
            Err(e) => {
                let why = format!("{url}: {e}");
                let cause = match &e {
                    ureq::Error::Timeout(_) => Cause::TimedOut,
                    ureq::Error::HostNotFound => Cause::NoName,
                    ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => Cause::Refused,
                    ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Cause::TimedOut,
                    _ => cause_of(&why),
                };
                last = (cause, why);
            }
        }
    }
    Err(last)
}

/// What became of work handed back (`Client::done`).
#[derive(Debug, PartialEq, Eq)]
pub enum Handed {
    Taken,
    Gone,
    Refused(String),
}

/// The lease calls a worker makes on the coordinator: work asked for, its lease kept alive, the work
/// handed back, failed or given back; the owner's cancel and pause passed on; the pool's beat.
pub trait LeaseCalls {
    /// Work that fits this worker; None when there's none now; an error, `Refused`, when none goes to
    /// it as it is.
    fn ask(&self, a: &Ask) -> Result<Option<Grant>>;

    /// Keeps lease `lease` alive; false when the coordinator no longer holds it for this worker (the
    /// work should stop: it was offered again).
    fn beat(&self, lease: u64, progress: Option<&str>) -> Result<bool>;

    /// `beat`, and the build's pause, while it's paused (the job pauses with it).
    fn beat_paused(&self, lease: u64, progress: Option<&str>) -> Result<(bool, Option<crate::control::Pause>)>;

    /// `beat_paused`, with what its job is doing now (crate::agent::jobs::Live), and the owner's
    /// cancel of the lease's job (`scenic job cancel`), when there's one: the job stops and gives
    /// the lease back. `slot`: the slot of this Mac running its job (crate::coord::lease::Lease::slot).
    fn beat_told(
        &self,
        lease: u64,
        progress: Option<&str>,
        live: Option<crate::agent::jobs::Live>,
        slot: Option<u32>,
    ) -> Result<(bool, Option<crate::control::Pause>, Option<crate::control::Cancel>)>;

    /// The member's beat listing the leases it holds (crate::coord::held, docs/coordination.md
    /// §6.1); None from a lead that doesn't take it (an older app's: 404).
    fn pool_beat(&self, b: &super::held::Beat) -> Result<Option<super::held::Answer>>;

    /// Passes on the owner's cancel (crate::control::Cancel) to the lead: the leases it told to stop,
    /// as (lease, worker, what).
    fn cancel(&self, c: &crate::control::Cancel) -> Result<Vec<(u64, String, String)>>;

    /// Passes on this Mac's ask (crate::control::Request): the build paused, or going on, as asked
    /// at `at` (unix seconds: an ask older than the build's last change is passed over).
    fn set_pause(&self, pause: Option<&crate::control::Pause>, at: u64) -> Result<()>;

    /// Hands work back: taken; gone (its lease ended: drop the work, it was offered again, and a
    /// late hand-off could undo a newer build); or refused (not what the lease asked for: give the
    /// lease back as failed, and drop the work).
    fn done(&self, d: &Done) -> Result<Handed>;

    /// Gives lease `lease` back, failed (`oom_mb`: a task out of memory at that peak).
    fn fail(&self, lease: u64, error: &str, oom_mb: Option<u64>) -> Result<()>;

    /// `fail`, with what the job saw its targets hold at least (crate::agent::memguard: cost key,
    /// MB), for the lease's floors.
    fn fail_with(
        &self,
        lease: u64,
        error: &str,
        oom_mb: Option<u64>,
        floors: &[(String, crate::agent::memguard::Floor)],
    ) -> Result<()>;

    /// Gives lease `lease` back unfinished, not failed (`why`: the build paused, the Mac slept, its
    /// agent restarted, it couldn't start here): its targets aren't kept from this worker.
    fn give_back(&self, lease: u64, why: &str) -> Result<()>;

    /// `give_back`, with what the job saw its targets hold at least (crate::agent::memguard: cost
    /// key, MB), for the lease's floors.
    fn give_back_with(&self, lease: u64, why: &str, floors: &[(String, crate::agent::memguard::Floor)]) -> Result<()>;

    /// Ends lease `lease` unfinished: given back (`interrupted`) or failed, with what the job saw its
    /// targets take at least, in memory (`floors`) and on the disk (`disks`: cost key, MB).
    fn end_with(
        &self,
        lease: u64,
        why: &str,
        interrupted: bool,
        floors: &[(String, crate::agent::memguard::Floor)],
        disks: &[(String, u64)],
        job_disk_mb: u64,
    ) -> Result<()>;
}

impl LeaseCalls for Client {
    fn ask(&self, a: &Ask) -> Result<Option<Grant>> {
        let a = Ask { worker: self.worker.clone(), ..a.clone() };
        match self.post_json("/work/ask", &serde_json::to_value(&a)?)? {
            (200, v) => Ok(Some(serde_json::from_value(v)?)),
            (409, v) => Err(Refused {
                why: v["error"].as_str().unwrap_or("the lead gives this Mac no work as it is").to_string(),
                pause: serde_json::from_value(v["pause"].clone()).ok().flatten(),
            }
            .into()),
            _ => Ok(None),
        }
    }

    fn beat(&self, lease: u64, progress: Option<&str>) -> Result<bool> {
        Ok(self.beat_paused(lease, progress)?.0)
    }

    fn beat_paused(&self, lease: u64, progress: Option<&str>) -> Result<(bool, Option<crate::control::Pause>)> {
        self.beat_told(lease, progress, None, None).map(|(alive, pause, _)| (alive, pause))
    }

    fn beat_told(
        &self,
        lease: u64,
        progress: Option<&str>,
        live: Option<crate::agent::jobs::Live>,
        slot: Option<u32>,
    ) -> Result<(bool, Option<crate::control::Pause>, Option<crate::control::Cancel>)> {
        let b = super::Beat {
            worker: self.worker.clone(),
            lease,
            progress: progress.map(str::to_string),
            frac: None,
            live,
            slot,
        };
        let v = self.post_json("/work/beat", &serde_json::to_value(&b)?)?.1;
        Ok((
            v["ok"].as_bool().unwrap_or(false),
            serde_json::from_value(v["pause"].clone()).ok().flatten(),
            serde_json::from_value(v["cancel"].clone()).ok().flatten(),
        ))
    }

    fn pool_beat(&self, b: &super::held::Beat) -> Result<Option<super::held::Answer>> {
        match self.post_json("/pool/beat", &serde_json::to_value(b)?)? {
            (404, _) => Ok(None),
            (_, v) => Ok(Some(serde_json::from_value(v)?)),
        }
    }

    fn cancel(&self, c: &crate::control::Cancel) -> Result<Vec<(u64, String, String)>> {
        let v = self.post_json("/work/cancel", &serde_json::to_value(c)?)?.1;
        let s = |t: &serde_json::Value, k: &str| t[k].as_str().unwrap_or("").to_string();
        Ok(v["told"]
            .as_array()
            .map(|a| a.iter().map(|t| (t["lease"].as_u64().unwrap_or(0), s(t, "worker"), s(t, "what"))).collect())
            .unwrap_or_default())
    }

    fn set_pause(&self, pause: Option<&crate::control::Pause>, at: u64) -> Result<()> {
        self.post_json("/work/pause", &serde_json::json!({ "pause": pause, "at": at }))?;
        Ok(())
    }

    fn done(&self, d: &Done) -> Result<Handed> {
        let mut v = serde_json::to_value(d)?;
        v["worker"] = self.worker.clone().into();
        let (code, b) = self.request("POST", "/work/done", Some(&serde_json::to_vec(&v)?))?;
        let why = || {
            serde_json::from_slice::<serde_json::Value>(&b)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_default()
        };
        match code {
            200 => Ok(Handed::Taken),
            410 => Ok(Handed::Gone),
            422 => Ok(Handed::Refused(why())),
            c => bail!("the coordinator answered {c} to /work/done: {}", why()),
        }
    }

    fn fail(&self, lease: u64, error: &str, oom_mb: Option<u64>) -> Result<()> {
        self.fail_with(lease, error, oom_mb, &[])
    }

    fn fail_with(
        &self,
        lease: u64,
        error: &str,
        oom_mb: Option<u64>,
        floors: &[(String, crate::agent::memguard::Floor)],
    ) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: error.chars().take(4000).collect(),
            oom_mb,
            interrupted: false,
            floors: floors.to_vec(),
            disks: Vec::new(),
            job_disk_mb: 0,
        };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    fn give_back(&self, lease: u64, why: &str) -> Result<()> {
        self.give_back_with(lease, why, &[])
    }

    fn give_back_with(&self, lease: u64, why: &str, floors: &[(String, crate::agent::memguard::Floor)]) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: why.chars().take(4000).collect(),
            oom_mb: None,
            interrupted: true,
            floors: floors.to_vec(),
            disks: Vec::new(),
            job_disk_mb: 0,
        };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    fn end_with(
        &self,
        lease: u64,
        why: &str,
        interrupted: bool,
        floors: &[(String, crate::agent::memguard::Floor)],
        disks: &[(String, u64)],
        job_disk_mb: u64,
    ) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: why.chars().take(4000).collect(),
            oom_mb: None,
            interrupted,
            floors: floors.to_vec(),
            disks: disks.to_vec(),
            job_disk_mb,
        };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coordwire::testkit::{dead, mute};
    use std::sync::atomic::Ordering;

    #[test]
    fn a_refused_connection_fails_the_round() {
        let c = Client::at(vec![dead()], "t".into(), "m");
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert!(c.round_failed());
        assert!(c.failed().is_some());
    }

    #[test]
    fn a_round_tries_an_unanswering_coordinator_once() {
        let (url, tried) = mute();
        let c = Client::at(vec![url], "t".into(), "m");
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 1);
        let e = c.post_json("/work/ask", &serde_json::json!({})).unwrap_err();
        assert!(format!("{e:#}").contains("not tried"), "{e:#}");
        assert!(c.done(&Done { lease: 1, ..Default::default() }).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 1, "the round's later requests not tried");
        // The next round tries again.
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn outside_rounds_every_request_tries() {
        let (url, tried) = mute();
        let c = Client::at(vec![url], "t".into(), "m");
        assert!(c.beat(1, None).is_err());
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 2);
    }
}
