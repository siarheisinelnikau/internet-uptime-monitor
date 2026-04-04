use crate::model::{Config, Sample, State, now};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;

#[derive(Default)]
struct Observation {
    last: Option<Sample>,
    span_id: i64,
    outage_id: Option<String>,
}

pub struct Store {
    db: Connection,
    observation: Observation,
    timeline_end: Option<f64>,
    prune_after: f64,
}

#[derive(Debug, Serialize)]
pub struct Span {
    pub started_at: f64,
    pub ended_at: f64,
    pub state: String,
}
#[derive(Debug, Serialize)]
pub struct Outage {
    pub id: String,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub last_seen_at: f64,
    pub cause: String,
    pub status: String,
    pub duration_seconds: f64,
    pub total_duration_seconds: f64,
    pub explanation: String,
}
#[derive(Debug, Serialize)]
pub struct Latency {
    pub bucket: i64,
    pub checked_at: f64,
    pub latency_ms: Option<f64>,
    pub checks: u64,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub start: f64,
    pub end: f64,
    pub monitored_seconds: f64,
    pub unmonitored_seconds: f64,
    pub uptime_percent: Option<f64>,
    pub observed_down_seconds: f64,
    pub downtime_seconds: f64,
    pub outage_count: usize,
    pub longest_outage_seconds: f64,
    pub average_latency_ms: Option<f64>,
    pub check_count: u64,
    pub minimum_outage_seconds: u32,
    pub outages: Vec<Outage>,
    pub outages_truncated: bool,
    pub spans: Vec<Span>,
    pub latency: Vec<Latency>,
}

fn interrupt(db: &Connection, observation: &mut Observation) -> Result<()> {
    if let Some(id) = &observation.outage_id {
        db.execute(
            "UPDATE outages SET ended_at=last_seen_at,status='interrupted' WHERE id=?",
            [id],
        )?;
    }
    *observation = Observation::default();
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(db: Connection) -> Result<Self> {
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
            CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1),value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS samples(id INTEGER PRIMARY KEY,checked_at REAL NOT NULL,state TEXT NOT NULL,latency_ms REAL,evidence TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS samples_time ON samples(checked_at);
            CREATE TABLE IF NOT EXISTS spans(id INTEGER PRIMARY KEY,started_at REAL NOT NULL,ended_at REAL NOT NULL,state TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS spans_time ON spans(ended_at);
            CREATE TABLE IF NOT EXISTS outages(id TEXT PRIMARY KEY,started_at REAL NOT NULL,ended_at REAL,last_seen_at REAL NOT NULL,cause TEXT NOT NULL,status TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS outages_time ON outages(last_seen_at);
            UPDATE outages SET ended_at=last_seen_at,status='interrupted' WHERE status='ongoing';
            PRAGMA user_version=1;")?;
        db.execute(
            "DELETE FROM samples WHERE checked_at<?",
            [now() - 30.0 * 86400.0],
        )?;
        let timeline_end = db.query_row("SELECT MAX(ended_at) FROM spans", [], |r| r.get(0))?;
        Ok(Self {
            db,
            observation: Observation::default(),
            timeline_end,
            prune_after: now() + 86400.0,
        })
    }
    pub fn config(&self) -> Result<Config> {
        let value: Option<String> = self
            .db
            .query_row("SELECT value FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let config = value
            .map(|v| serde_json::from_str::<Config>(&v))
            .transpose()?
            .unwrap_or_default();
        config.validate()?;
        Ok(config)
    }
    pub fn save_config(&self, config: &Config) -> Result<()> {
        config.validate()?;
        self.db.execute(
            "INSERT OR REPLACE INTO settings VALUES(1,?)",
            [serde_json::to_string(config)?],
        )?;
        Ok(())
    }
    pub fn interrupt(&mut self) -> Result<()> {
        let tx = self.db.transaction()?;
        interrupt(&tx, &mut self.observation)?;
        tx.commit()?;
        Ok(())
    }
    pub fn latest(&self) -> Result<Option<Sample>> {
        let value: Option<String> = self
            .db
            .query_row(
                "SELECT evidence FROM samples ORDER BY checked_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(value.map(|v| serde_json::from_str(&v)).transpose()?)
    }
    pub fn record(&mut self, sample: &Sample, gap_limit: f64) -> Result<()> {
        let timestamp = sample.checked_at;
        let tx = self.db.transaction()?;
        let obs = &mut self.observation;
        if self.timeline_end.is_some_and(|end| timestamp <= end) {
            interrupt(&tx, obs)?;
            tx.commit()?;
            return Ok(());
        }
        if obs
            .last
            .as_ref()
            .is_some_and(|last| timestamp - last.checked_at > gap_limit)
        {
            interrupt(&tx, obs)?;
        }
        if obs.last.is_some() {
            tx.execute(
                "UPDATE spans SET ended_at=? WHERE id=?",
                params![timestamp, obs.span_id],
            )?;
        }
        if obs
            .last
            .as_ref()
            .is_none_or(|last| last.state != sample.state)
        {
            tx.execute(
                "INSERT INTO spans(started_at,ended_at,state) VALUES(?,?,?)",
                params![timestamp, timestamp, sample.state.as_str()],
            )?;
            obs.span_id = tx.last_insert_rowid();
        }
        if sample.state.is_down() {
            if let Some(id) = &obs.outage_id {
                tx.execute("UPDATE outages SET last_seen_at=?,cause=CASE WHEN cause=? THEN cause ELSE 'mixed' END WHERE id=?", params![timestamp, sample.state.as_str(), id])?;
            } else {
                let id = format!("{timestamp:.6}-{}", obs.span_id);
                tx.execute(
                    "INSERT INTO outages VALUES(?,?,NULL,?,?,'ongoing')",
                    params![id, timestamp, timestamp, sample.state.as_str()],
                )?;
                obs.outage_id = Some(id);
            }
        } else if let Some(id) = obs.outage_id.take() {
            if sample.state == State::Online {
                tx.execute(
                    "UPDATE outages SET ended_at=?,last_seen_at=?,status='recovered' WHERE id=?",
                    params![timestamp, timestamp, id],
                )?;
            } else {
                tx.execute(
                    "UPDATE outages SET ended_at=last_seen_at,status='interrupted' WHERE id=?",
                    [id],
                )?;
            }
        }
        let latency = sample
            .targets
            .iter()
            .filter(|t| t.ok == Some(true))
            .filter_map(|t| t.latency_ms)
            .reduce(f64::min);
        tx.execute(
            "INSERT INTO samples(checked_at,state,latency_ms,evidence) VALUES(?,?,?,?)",
            params![
                timestamp,
                sample.state.as_str(),
                latency,
                serde_json::to_string(sample)?
            ],
        )?;
        if timestamp >= self.prune_after {
            tx.execute(
                "DELETE FROM samples WHERE checked_at<?",
                [timestamp - 30.0 * 86400.0],
            )?;
            self.prune_after = timestamp + 86400.0;
        }
        tx.commit()?;
        obs.last = Some(sample.clone());
        self.timeline_end = Some(timestamp);
        Ok(())
    }
    pub fn report(&self, start: f64, end: f64, minimum: u32, limit: usize) -> Result<Report> {
        let mut statement = self.db.prepare("SELECT started_at,ended_at,state FROM spans WHERE ended_at>? AND started_at<? ORDER BY started_at")?;
        let spans = statement
            .query_map(params![start, end], |r| {
                Ok(Span {
                    started_at: r.get::<_, f64>(0)?.max(start),
                    ended_at: r.get::<_, f64>(1)?.min(end),
                    state: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let monitored: f64 = spans
            .iter()
            .filter(|s| s.state != "unknown")
            .map(|s| s.ended_at - s.started_at)
            .sum();
        let online: f64 = spans
            .iter()
            .filter(|s| s.state == "online")
            .map(|s| s.ended_at - s.started_at)
            .sum();
        let mut statement = self.db.prepare("SELECT * FROM outages WHERE last_seen_at>=? AND started_at<? AND last_seen_at-started_at>=? ORDER BY started_at DESC")?;
        let mut outages = statement
            .query_map(params![start, end, minimum], |r| {
                let began: f64 = r.get(1)?;
                let ended: Option<f64> = r.get(2)?;
                let last: f64 = r.get(3)?;
                let cause: String = r.get(4)?;
                let explanation = match cause.as_str() {
                    "upstream" => "Internet unreachable; router responded",
                    "local" => "Internet and router unreachable",
                    "mixed" => "Router reachability changed during the outage",
                    _ => "Internet unreachable; router status unknown",
                }
                .to_string();
                Ok(Outage {
                    id: r.get(0)?,
                    started_at: began,
                    ended_at: ended,
                    last_seen_at: last,
                    cause,
                    status: r.get(5)?,
                    duration_seconds: (ended.unwrap_or(last).min(end) - began.max(start)).max(0.0),
                    total_duration_seconds: ended.unwrap_or(last) - began,
                    explanation,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        outages.retain(|e| e.duration_seconds > 0.0);
        let outage_count = outages.len();
        let downtime_seconds = outages.iter().map(|e| e.duration_seconds).sum();
        let longest_outage_seconds = outages
            .iter()
            .map(|e| e.duration_seconds)
            .fold(0.0, f64::max);
        let truncated = limit > 0 && outages.len() > limit;
        if limit > 0 {
            outages.truncate(limit);
        }
        let mut statement = self.db.prepare("SELECT CAST((checked_at-?)/? AS INTEGER),AVG(checked_at),AVG(latency_ms),COUNT(*) FROM samples WHERE checked_at>=? AND checked_at<? GROUP BY 1 ORDER BY 1")?;
        let latency = statement
            .query_map(
                params![start, ((end - start) / 120.0).max(1.0), start, end],
                |r| {
                    Ok(Latency {
                        bucket: r.get(0)?,
                        checked_at: r.get(1)?,
                        latency_ms: r.get(2)?,
                        checks: r.get(3)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (average_latency_ms, check_count) = self.db.query_row(
            "SELECT AVG(latency_ms),COUNT(*) FROM samples WHERE checked_at>=? AND checked_at<?",
            params![start, end],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(Report {
            start,
            end,
            monitored_seconds: monitored,
            unmonitored_seconds: (end - start - monitored).max(0.0),
            uptime_percent: (monitored > 0.0).then(|| online / monitored * 100.0),
            observed_down_seconds: (monitored - online).max(0.0),
            downtime_seconds,
            outage_count,
            longest_outage_seconds,
            average_latency_ms,
            check_count,
            minimum_outage_seconds: minimum,
            outages,
            outages_truncated: truncated,
            spans,
            latency,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Probe, classify};
    pub fn sample(timestamp: f64, state: State) -> Sample {
        let ok = if state == State::Unknown {
            None
        } else {
            Some(state == State::Online)
        };
        Sample {
            checked_at: timestamp,
            state,
            router: Probe {
                name: None,
                address: Some("192.168.1.1".into()),
                ok: Some(state != State::Local),
                latency_ms: Some(1.0),
                error: None,
            },
            targets: vec![Probe {
                name: Some("Test".into()),
                address: Some("1.1.1.1".into()),
                ok,
                latency_ms: if ok == Some(true) { Some(20.0) } else { None },
                error: None,
            }],
        }
    }
    fn recorded(events: &[(f64, State)]) -> Store {
        let mut store = Store::memory().unwrap();
        for &(t, s) in events {
            store.record(&sample(t, s), 30.0).unwrap();
        }
        store
    }
    #[test]
    fn empty_history_has_no_invented_uptime() {
        let r = Store::memory().unwrap().report(100.0, 200.0, 5, 0).unwrap();
        assert!(r.uptime_percent.is_none());
        assert_eq!(r.unmonitored_seconds, 100.0);
    }
    #[test]
    fn recovery_and_downtime_are_timed() {
        let store = recorded(&[
            (100.0, State::Online),
            (105.0, State::Upstream),
            (110.0, State::Upstream),
            (115.0, State::Online),
            (120.0, State::Online),
        ]);
        let r = store.report(100.0, 130.0, 5, 0).unwrap();
        assert_eq!(r.outage_count, 1);
        assert_eq!(r.downtime_seconds, 10.0);
        assert_eq!(r.monitored_seconds, 20.0);
        assert_eq!(r.uptime_percent, Some(50.0));
        assert_eq!(r.outages[0].status, "recovered");
    }
    #[test]
    fn sleep_is_unmonitored_not_an_outage() {
        let store = recorded(&[
            (100.0, State::Upstream),
            (110.0, State::Upstream),
            (1000.0, State::Online),
            (1010.0, State::Online),
        ]);
        let r = store.report(100.0, 1020.0, 5, 0).unwrap();
        assert_eq!(r.downtime_seconds, 10.0);
        assert_eq!(r.monitored_seconds, 20.0);
        assert_eq!(r.outages[0].status, "interrupted");
    }
    #[test]
    fn cause_changes_merge_without_double_counting() {
        let store = recorded(&[
            (100.0, State::Local),
            (105.0, State::Upstream),
            (110.0, State::Online),
        ]);
        let r = store.report(90.0, 120.0, 5, 0).unwrap();
        assert_eq!(r.outage_count, 1);
        assert_eq!(r.outages[0].cause, "mixed");
        assert_eq!(r.downtime_seconds, 10.0);
    }
    #[test]
    fn clips_events_at_period_boundaries() {
        let store = recorded(&[
            (95.0, State::Upstream),
            (105.0, State::Upstream),
            (115.0, State::Online),
        ]);
        let r = store.report(100.0, 110.0, 5, 0).unwrap();
        assert_eq!(r.downtime_seconds, 10.0);
        assert_eq!(r.outages[0].total_duration_seconds, 20.0);
    }
    #[test]
    fn short_interruptions_still_affect_availability() {
        let store = recorded(&[
            (100.0, State::Online),
            (102.0, State::Upstream),
            (104.0, State::Online),
            (110.0, State::Online),
        ]);
        let r = store.report(100.0, 111.0, 5, 0).unwrap();
        assert_eq!(r.outage_count, 0);
        assert_eq!(r.observed_down_seconds, 2.0);
        assert_eq!(r.uptime_percent, Some(80.0));
    }
    #[test]
    fn unknown_is_excluded_from_availability() {
        let store = recorded(&[
            (100.0, State::Unknown),
            (110.0, State::Online),
            (115.0, State::Online),
        ]);
        let r = store.report(100.0, 120.0, 5, 0).unwrap();
        assert_eq!(r.monitored_seconds, 5.0);
        assert_eq!(r.uptime_percent, Some(100.0));
        assert_eq!(r.outage_count, 0);
    }
    #[test]
    fn paused_sessions_never_join() {
        let mut store = recorded(&[(100.0, State::Online), (105.0, State::Online)]);
        store.interrupt().unwrap();
        store.record(&sample(120.0, State::Online), 30.0).unwrap();
        store.record(&sample(125.0, State::Online), 30.0).unwrap();
        assert_eq!(
            store.report(100.0, 130.0, 5, 0).unwrap().monitored_seconds,
            10.0
        );
    }
    #[test]
    fn restart_recovers_interrupted_events_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        {
            let mut store = Store::open(&path).unwrap();
            store.record(&sample(100.0, State::Upstream), 30.0).unwrap();
            store.record(&sample(110.0, State::Upstream), 30.0).unwrap();
            store
                .save_config(&Config {
                    interval_seconds: 10,
                    ..Config::default()
                })
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.config().unwrap().interval_seconds, 10);
        let r = store.report(90.0, 120.0, 5, 0).unwrap();
        assert_eq!(r.outages[0].status, "interrupted");
        assert_eq!(r.downtime_seconds, 10.0);
    }
    #[test]
    fn backwards_clock_does_not_duplicate_coverage() {
        let store = recorded(&[
            (100.0, State::Online),
            (105.0, State::Online),
            (90.0, State::Online),
            (95.0, State::Online),
            (106.0, State::Online),
        ]);
        assert_eq!(
            store.report(80.0, 110.0, 5, 0).unwrap().monitored_seconds,
            5.0
        );
    }
    #[test]
    fn sample_evidence_roundtrips() {
        let mut store = Store::memory().unwrap();
        let s = sample(now(), State::Online);
        store.record(&s, 30.0).unwrap();
        let saved = store.latest().unwrap().unwrap();
        assert!((saved.checked_at - s.checked_at).abs() < 0.00001);
        assert_eq!(classify(&saved.targets, &saved.router), State::Online);
    }
}
