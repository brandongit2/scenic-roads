//! A job run's id (docs/pool.md §7.5, Job ids): the id of the lease the lead's coordinator first
//! granted the run under, `<epoch>-<n>`: the epoch of the lead's store it was granted in and the
//! lease's number. One run, one id: a job its agent's successor takes over keeps it, and so does a
//! job whose lease is held again under a new number (agentcore's coord::lease::Lease::job); a job
//! paused and resumed, failed and tried again, is a new run under a new lease.
//!
//! Never given twice: the epoch goes up at each start of the lead's store and past every epoch a
//! take-up finds (agentcore's pool::driver::Io::epoch), and within an epoch the store's numbers
//! only count up (agentcore's coord::lease::Leases: from past the time in milliseconds, the saved
//! leases, those taken up and the highest its history names, `load`, `restore`, `past`). Epoch 0 is
//! no lead's: a coordinator out of the pool, a hand-off drained from one, and a history event from
//! before job ids, which named its run by the lease's number alone (agentcore's
//! coord::history::Event::job: read as `0-<n>`).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A job run's id, `<epoch>-<n>`, ordered by both (later grants are greater).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId {
    /// The epoch its lease was granted in (0: none, see the module's doc).
    pub epoch: u64,
    /// Its lease's number.
    pub n: u64,
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.epoch, self.n)
    }
}

impl FromStr for JobId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<JobId> {
        let Some((t, n)) = s.split_once('-') else { bail!("{s:?} isn't a job id: <epoch>-<n>") };
        match (t.parse(), n.parse()) {
            (Ok(epoch), Ok(n)) => Ok(JobId { epoch, n }),
            _ => bail!("{s:?} isn't a job id: <epoch>-<n>"),
        }
    }
}

impl Serialize for JobId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<JobId, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_read_back_and_order_by_epoch_then_number() {
        let l: JobId = "12-345".parse().unwrap();
        assert_eq!(l, JobId { epoch: 12, n: 345 });
        assert_eq!(serde_json::to_string(&l).unwrap(), "\"12-345\"");
        assert_eq!(serde_json::from_str::<JobId>("\"12-345\"").unwrap(), l);
        assert!("12".parse::<JobId>().is_err() && "a-1".parse::<JobId>().is_err());
        let mut v = [JobId { epoch: 3, n: 1 }, JobId { epoch: 2, n: 10 }, JobId { epoch: 2, n: 9 }];
        v.sort();
        assert_eq!(v.map(|l| l.to_string()), ["2-9", "2-10", "3-1"]);
        // (A lease's number alone is no job id: only a history event from before them names one so,
        // agentcore's coord::history::Event::job.)
        assert!(serde_json::from_str::<JobId>("1791328399374").is_err());
    }
}
