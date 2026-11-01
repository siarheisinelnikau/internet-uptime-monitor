use crate::storage::Report;

pub fn iso(timestamp: f64) -> String {
    chrono::DateTime::from_timestamp(timestamp.floor() as i64, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
pub fn text_report(report: &Report, demo: bool) -> String {
    text_report_localized(report, demo, crate::i18n::Language::English)
}
/// Human-readable exports follow the interface language; CSV/JSON keep stable keys.
pub fn text_report_localized(
    report: &Report,
    demo: bool,
    language: crate::i18n::Language,
) -> String {
    use crate::i18n::{diagnostic, tr};
    let t = |key| tr(language, key);
    let mut text = format!(
        "{}\n{}\n\n{}: {} — {}\n",
        t("INTERNET UPTIME MONITOR · CONNECTION REPORT"),
        t(if demo {
            "DEMO DATA — not real measurements"
        } else {
            "Local ICMP observations"
        }),
        t("Period (UTC)"),
        iso(report.start),
        iso(report.end)
    );
    for (label, value) in [
        ("Outages", report.outage_count.to_string()),
        (
            "Total downtime",
            format!("{:.1}{}", report.downtime_seconds, t(" seconds")),
        ),
        (
            "Monitored time",
            format!("{:.1}{}", report.monitored_seconds, t(" seconds")),
        ),
        (
            "Unmonitored time",
            format!("{:.1}{}", report.unmonitored_seconds, t(" seconds")),
        ),
        (
            "Connection availability",
            report
                .uptime_percent
                .map(|p| format!("{p:.3}%"))
                .unwrap_or_else(|| t("No monitored time").into()),
        ),
        (
            "Outage threshold",
            format!("{}{}", report.minimum_outage_seconds, t(" seconds")),
        ),
    ] {
        text.push_str(&format!("{}: {value}\n", t(label)));
    }
    text.push_str(t("Availability includes shorter interruptions."));
    text.push_str(&format!(
        "\n\n{} | {} | {} | {} | {}\n",
        t("Started (UTC)"),
        t("Ended (UTC)"),
        t("Seconds in period"),
        t("Status"),
        t("Evidence")
    ));
    for event in &report.outages {
        let state = match event.status.as_str() {
            "recovered" => "Recovered",
            "ongoing" => "Ongoing",
            _ => "Observation ended",
        };
        text.push_str(&format!(
            "{} | {} | {:.1} | {} | {}\n",
            iso(event.started_at),
            event
                .ended_at
                .map(iso)
                .unwrap_or_else(|| t("Ongoing").into()),
            event.duration_seconds,
            t(state),
            diagnostic(language, &event.explanation)
        ));
    }
    text.push('\n');
    for key in [
        "A responding router does not prove ISP responsibility.",
        "Exports use UTC timestamps and include monitoring coverage. ICMP filtering can look like an outage; DNS, HTTPS, and bandwidth are not tested.",
        "Times and durations are approximate to the sampling interval. Sleep, pauses, and stopped monitoring are unmonitored.",
        "Interrupted means observation stopped before recovery was confirmed. Router evidence suggests a location; it does not prove ISP responsibility.",
    ] {
        text.push_str(t(key));
        text.push('\n');
    }
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
