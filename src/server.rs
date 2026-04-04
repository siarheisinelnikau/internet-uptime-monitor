use crate::{
    model::{Config, now},
    monitor::App,
    storage::Report,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    sync::Arc,
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

type Reply = Response<Cursor<Vec<u8>>>;
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'";

fn response(
    body: impl Into<Vec<u8>>,
    content_type: &str,
    code: u16,
    filename: Option<&str>,
) -> Reply {
    let mut reply = Response::from_data(body.into()).with_status_code(StatusCode(code));
    for (key, value) in [
        ("Content-Type", format!("{content_type}; charset=utf-8")),
        ("Cache-Control", "no-store".into()),
        ("X-Content-Type-Options", "nosniff".into()),
        ("Content-Security-Policy", CSP.into()),
    ] {
        reply.add_header(Header::from_bytes(key, value).expect("constant HTTP header"));
    }
    if let Some(name) = filename {
        reply.add_header(
            Header::from_bytes(
                "Content-Disposition",
                format!("attachment; filename=\"{name}\""),
            )
            .unwrap(),
        );
    }
    reply
}
fn json_reply(value: &Value, code: u16) -> Reply {
    response(
        serde_json::to_vec(value).unwrap(),
        "application/json",
        code,
        None,
    )
}
fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}
pub fn allowed(host: Option<&str>, origin: Option<&str>, port: u16) -> bool {
    let authorities = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    host.is_some_and(|h| authorities.iter().any(|a| a == h))
        && origin.is_none_or(|o| authorities.iter().any(|a| o == format!("http://{a}")))
}
fn query(url: &str) -> HashMap<&str, &str> {
    url.split_once('?')
        .map(|(_, s)| {
            s.split('&')
                .filter_map(|pair| pair.split_once('='))
                .collect()
        })
        .unwrap_or_default()
}
pub fn period(start: Option<&str>, end: Option<&str>, now: f64) -> Result<(f64, f64)> {
    let start = start
        .map(str::parse::<f64>)
        .transpose()?
        .unwrap_or(now - 86400.0);
    let requested_end = end.map(str::parse::<f64>).transpose()?.unwrap_or(now);
    let end = requested_end.min(now);
    if !start.is_finite()
        || !requested_end.is_finite()
        || end <= start
        || end - start > 366.0 * 86400.0
    {
        bail!("Choose a valid period of up to 366 days.");
    }
    Ok((start, end))
}
pub fn iso(timestamp: f64) -> String {
    chrono::DateTime::from_timestamp(timestamp.floor() as i64, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
pub fn text_report(report: &Report, demo: bool) -> String {
    let availability = report
        .uptime_percent
        .map(|p| format!("{p:.3}% of monitored time"))
        .unwrap_or_else(|| "no monitored time".into());
    let mut text = format!(
        "INTERNET UPTIME MONITOR — CONNECTION REPORT\n{}\n\nPeriod (UTC): {} to {}\n{} outages, {:.1} seconds of downtime.\nMonitored: {:.1} seconds; unmonitored: {:.1} seconds.\nAvailability: {availability}.\nOutage threshold: {} seconds. Availability includes shorter interruptions.\n\nStarted (UTC) | Ended (UTC) | Seconds in period | Status | Evidence\n",
        if demo {
            "DEMO DATA — not real measurements"
        } else {
            "Local ICMP observations"
        },
        iso(report.start),
        iso(report.end),
        report.outage_count,
        report.downtime_seconds,
        report.monitored_seconds,
        report.unmonitored_seconds,
        report.minimum_outage_seconds
    );
    for event in &report.outages {
        text.push_str(&format!(
            "{} | {} | {:.1} | {} | {}\n",
            iso(event.started_at),
            event.ended_at.map(iso).unwrap_or_else(|| "Ongoing".into()),
            event.duration_seconds,
            event.status,
            event.explanation
        ));
    }
    text.push_str("\nA responding router with unreachable public targets suggests an upstream issue; it does not prove ISP fault.\nICMP filtering can resemble an outage. DNS, HTTPS, speed, and application performance are not tested.\nTimes and durations are approximate to the sampling interval. Sleep, pauses, and stopped monitoring are unmonitored.\nInterrupted means observation stopped before recovery was confirmed.\n");
    text
}
fn csv_report(report: &Report, demo: bool) -> String {
    let mut text = "started_utc,ended_utc,seconds_in_period,status,evidence,demo\r\n".to_string();
    for e in &report.outages {
        text.push_str(&format!(
            "{},{},{:.2},{},\"{}\",{}\r\n",
            iso(e.started_at),
            e.ended_at.map(iso).unwrap_or_default(),
            e.duration_seconds,
            e.status,
            e.explanation.replace('"', "\"\""),
            demo
        ));
    }
    text
}
fn route(request: &mut Request, app: &Arc<App>, port: u16) -> Result<Reply> {
    if !allowed(header(request, "Host"), header(request, "Origin"), port) {
        return Ok(json_reply(
            &json!({"error":"This dashboard only accepts same-origin localhost requests."}),
            403,
        ));
    }
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    if request.method() == &Method::Get {
        if path == "/api/health" {
            return Ok(json_reply(
                &json!({"ok":true,"application":"internet-uptime-monitor","demo":app.snapshot().demo,"version":crate::updates::VERSION,"pid":std::process::id()}),
                200,
            ));
        }
        if path == "/api/updates" {
            return Ok(json_reply(
                &serde_json::to_value(app.updates.snapshot())?,
                200,
            ));
        }
        if path == "/api/dashboard" || path == "/api/export" {
            let query = query(&url);
            let (start, end) = period(
                query.get("start").copied(),
                query.get("end").copied(),
                now(),
            )?;
            let status = app.snapshot();
            let report = app.store.lock().expect("store lock").report(
                start,
                end,
                status.config.minimum_outage_seconds,
                if path == "/api/export" { 0 } else { 500 },
            )?;
            if path == "/api/dashboard" {
                return Ok(json_reply(
                    &json!({"status":status,"report":report,"server_time":now(),"updates":app.updates.snapshot()}),
                    200,
                ));
            }
            let stamp = iso(start).chars().take(10).collect::<String>();
            let format = query.get("format").copied().unwrap_or("txt");
            let filename = format!("uptime-{stamp}.{format}");
            return Ok(match format {
                "txt" => response(
                    text_report(&report, status.demo),
                    "text/plain",
                    200,
                    Some(&filename),
                ),
                "csv" => response(
                    csv_report(&report, status.demo),
                    "text/csv",
                    200,
                    Some(&filename),
                ),
                "json" => {
                    let mut value = serde_json::to_value(&report)?;
                    value["demo"] = json!(status.demo);
                    response(
                        serde_json::to_vec_pretty(&value)?,
                        "application/json",
                        200,
                        Some(&filename),
                    )
                }
                _ => bail!("Choose txt, csv, or json."),
            });
        }
        let asset: Option<(&[u8], &str)> = match path {
            "/" | "/index.html" => Some((include_bytes!("../ui/index.html"), "text/html")),
            "/app.js" => Some((include_bytes!("../ui/app.js"), "text/javascript")),
            "/style.css" => Some((include_bytes!("../ui/style.css"), "text/css")),
            "/icon.svg" => Some((include_bytes!("../ui/icon.svg"), "image/svg+xml")),
            _ => None,
        };
        return Ok(asset
            .map(|(bytes, content)| response(bytes, content, 200, None))
            .unwrap_or_else(|| json_reply(&json!({"error":"Not found."}), 404)));
    }
    if request.method() != &Method::Post {
        return Ok(json_reply(&json!({"error":"Method not allowed."}), 405));
    }
    if header(request, "Content-Type").and_then(|s| s.split(';').next()) != Some("application/json")
    {
        return Ok(json_reply(&json!({"error":"Send application/json."}), 415));
    }
    if request.body_length().is_none_or(|n| n == 0 || n > 16384) {
        bail!("Request body must contain 1–16384 bytes.");
    }
    let mut bytes = Vec::new();
    request.as_reader().take(16385).read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        bail!("Request body too large.");
    }
    let body: Value = serde_json::from_slice(&bytes)?;
    match path {
        "/api/settings" => {
            if !body.is_object() {
                bail!("Submit monitor settings as a JSON object.");
            }
            app.configure(serde_json::from_value::<Config>(body)?)?;
        }
        "/api/control" => app.control(body["action"].as_str().unwrap_or(""))?,
        "/api/shutdown" => app.stop(),
        "/api/updates/check" => app.updates.check()?,
        "/api/updates/install" => app.updates.install(app)?,
        _ => return Ok(json_reply(&json!({"error":"Not found."}), 404)),
    }
    Ok(json_reply(&json!({"ok":true}), 200))
}

pub fn bind(port: u16) -> Result<Server> {
    Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("Could not open localhost port {port}: {e}"))
}
pub fn start(server: Server, app: &Arc<App>) -> Vec<thread::JoinHandle<()>> {
    let port = server.server_addr().to_ip().expect("IP listener").port();
    let server = Arc::new(server);
    (0..4)
        .map(|_| {
            let server = server.clone();
            let app = app.clone();
            thread::spawn(move || {
                while !app.is_stopping() {
                    match server.recv_timeout(Duration::from_millis(200)) {
                        Ok(Some(mut request)) => {
                            let reply = route(&mut request, &app, port).unwrap_or_else(|e| {
                                json_reply(&json!({"error":e.to_string()}), 400)
                            });
                            let _ = request.respond(reply);
                        }
                        Ok(None) => {}
                        Err(_) => {
                            app.stop();
                            break;
                        }
                    }
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store;
    #[test]
    fn rejects_rebinding_and_cross_origin_requests() {
        assert!(allowed(Some("127.0.0.1:8765"), None, 8765));
        assert!(allowed(
            Some("localhost:8765"),
            Some("http://localhost:8765"),
            8765
        ));
        assert!(!allowed(Some("attacker.example"), None, 8765));
        assert!(!allowed(
            Some("127.0.0.1:8765"),
            Some("https://attacker.example"),
            8765
        ));
        assert!(!allowed(None, None, 8765));
    }
    #[test]
    fn rejects_invalid_report_ranges() {
        for (s, e) in [
            (Some("nan"), None),
            (None, Some("NaN")),
            (Some("-inf"), None),
            (Some("1"), Some("0")),
            (Some("1"), None),
        ] {
            assert!(period(s, e, now()).is_err());
        }
    }
    #[test]
    fn empty_export_never_claims_full_day_uptime() {
        let r = Store::memory()
            .unwrap()
            .report(now() - 86400.0, now(), 5, 0)
            .unwrap();
        assert!(text_report(&r, false).contains("no monitored time"));
        assert!(text_report(&r, true).contains("DEMO DATA"));
        assert!(csv_report(&r, false).starts_with("started_utc,ended_utc"));
    }
}
