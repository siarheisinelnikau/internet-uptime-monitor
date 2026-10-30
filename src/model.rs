use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub check_updates_automatically: bool,
    pub interval_seconds: u32,
    pub timeout_ms: u32,
    pub minimum_outage_seconds: u32,
    pub router: String,
    pub targets: Vec<Target>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check_updates_automatically: true,
            interval_seconds: 5,
            timeout_ms: 1000,
            minimum_outage_seconds: 5,
            router: String::new(),
            targets: [
                ("Cloudflare", "1.1.1.1"),
                ("Google", "8.8.8.8"),
                ("Quad9", "9.9.9.9"),
            ]
            .map(|(name, address)| Target {
                name: name.into(),
                address: address.into(),
            })
            .into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if !(2..=300).contains(&self.interval_seconds) {
            bail!("Check interval must be 2–300 seconds.");
        }
        if !(200..=5000).contains(&self.timeout_ms) {
            bail!("Response timeout must be 200–5000 ms.");
        }
        if self.timeout_ms >= self.interval_seconds * 1000 {
            bail!("Response timeout must be shorter than the check interval.");
        }
        if self.minimum_outage_seconds > 300 {
            bail!("Minimum outage length must be 0–300 seconds.");
        }
        if !self.router.is_empty() {
            validate_address(&self.router)?;
        }
        if !(2..=5).contains(&self.targets.len()) {
            bail!("Use two to five independent public targets.");
        }
        let mut addresses = std::collections::HashSet::new();
        for target in &self.targets {
            if target.name.trim().is_empty() || target.name.chars().count() > 40 {
                bail!("Target names must contain 1–40 characters.");
            }
            validate_address(&target.address)?;
            if !addresses.insert(&target.address) {
                bail!("Internet targets must use different IPv4 addresses.");
            }
        }
        Ok(())
    }
}

pub fn validate_address(value: &str) -> Result<Ipv4Addr> {
    let address: Ipv4Addr = value
        .parse()
        .map_err(|_| anyhow::anyhow!("Use a valid IPv4 address, such as 192.168.1.1."))?;
    if address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address.is_broadcast()
    {
        bail!("Use a reachable unicast IPv4 address.");
    }
    Ok(address)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Online,
    Upstream,
    Local,
    Undetermined,
    Unknown,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Upstream => "upstream",
            Self::Local => "local",
            Self::Undetermined => "undetermined",
            Self::Unknown => "unknown",
        }
    }
    pub fn is_down(self) -> bool {
        matches!(self, Self::Upstream | Self::Local | Self::Undetermined)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub address: Option<String>,
    pub ok: Option<bool>,
    pub latency_ms: Option<f64>,
    pub error: Option<String>,
}

impl Probe {
    pub fn unknown(address: Option<String>, message: impl Into<String>) -> Self {
        Self {
            name: None,
            address,
            ok: None,
            latency_ms: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub checked_at: f64,
    pub state: State,
    pub router: Probe,
    pub targets: Vec<Probe>,
}

pub fn classify(targets: &[Probe], router: &Probe) -> State {
    if targets.iter().any(|t| t.ok == Some(true)) {
        State::Online
    } else if targets.iter().any(|t| t.ok.is_none()) {
        State::Unknown
    } else {
        match router.ok {
            Some(true) => State::Upstream,
            Some(false) => State::Local,
            None => State::Undetermined,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub paused: bool,
    pub latest: Option<Sample>,
    pub error: Option<String>,
    pub gateway: Option<String>,
    pub config: Config,
    pub demo: bool,
    #[serde(skip)]
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn probe(ok: Option<bool>) -> Probe {
        Probe {
            ok,
            name: None,
            address: None,
            latency_ms: None,
            error: None,
        }
    }
    #[test]
    fn multi_target_classification() {
        assert_eq!(
            classify(
                &[probe(Some(false)), probe(Some(true)), probe(None)],
                &probe(None)
            ),
            State::Online
        );
        assert_eq!(
            classify(
                &[probe(Some(false)), probe(Some(false))],
                &probe(Some(true))
            ),
            State::Upstream
        );
        assert_eq!(
            classify(
                &[probe(Some(false)), probe(Some(false))],
                &probe(Some(false))
            ),
            State::Local
        );
        assert_eq!(
            classify(&[probe(Some(false)), probe(Some(false))], &probe(None)),
            State::Undetermined
        );
        assert_eq!(
            classify(&[probe(Some(false)), probe(None)], &probe(Some(true))),
            State::Unknown
        );
    }
    #[test]
    fn validates_settings_before_probing() {
        let config = Config::default();
        assert!(config.validate().is_ok());
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap(),
            config
        );
        for address in [
            "127.0.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "1.1.1.1;bad",
        ] {
            assert!(validate_address(address).is_err());
        }
        assert!(
            Config {
                interval_seconds: 1,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                interval_seconds: 2,
                timeout_ms: 2000,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                targets: vec![config.targets[0].clone(); 2],
                ..config
            }
            .validate()
            .is_err()
        );
        assert!(serde_json::from_str::<Config>(r#"{"interval_seconds":true}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"extra":true}"#).is_err());
    }
}
