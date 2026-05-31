use crate::storage::Report;

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
pub fn csv_report(report: &Report, demo: bool) -> String {
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
