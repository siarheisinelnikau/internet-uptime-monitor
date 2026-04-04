"use strict";

const $ = (id) => document.getElementById(id);
const svgNS = "http://www.w3.org/2000/svg";
const titles = {
  overview: [
    "Overview",
    "Your internet, on record.",
    "Know when it dropped. Know how long. Have the evidence.",
  ],
  history: [
    "Outage history",
    "Every disconnect, documented.",
    "A connection diary with the evidence behind each interruption.",
  ],
  reports: [
    "Reports",
    "A clear story. Ready to share.",
    "Turn those frustrating moments into a report you can use.",
  ],
  settings: [
    "Settings",
    "Make yourself comfortable.",
    "A few simple preferences. Then let the monitor do its thing.",
  ],
};
const colors = {
  online: "#90b77a",
  upstream: "#e7a474",
  local: "#cb7774",
  undetermined: "#c5ab89",
  unknown: "#e8ece5",
};
let data = null,
  loading = false,
  refreshRequested = false,
  settingsLoaded = false,
  updateRestarting = false,
  expectedUpdate = null,
  toastTimer;

function el(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (className) element.className = className;
  return element;
}
function svgEl(tag, attrs = {}) {
  const element = document.createElementNS(svgNS, tag);
  for (const [key, value] of Object.entries(attrs))
    element.setAttribute(key, value);
  return element;
}
function icon(name) {
  const result = svgEl("svg", { "aria-hidden": "true" });
  result.append(svgEl("use", { href: `#i-${name}` }));
  return result;
}
function duration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}
function timeLabel(timestamp, includeDate = false) {
  return new Date(timestamp * 1000).toLocaleString("en-GB", {
    ...(includeDate ? { month: "short", day: "numeric" } : {}),
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
}
function period() {
  const now = new Date(),
    start = new Date(now);
  if ($("period").value === "today") start.setHours(0, 0, 0, 0);
  else start.setDate(start.getDate() - Number($("period").value));
  return { start: start.getTime() / 1000, end: now.getTime() / 1000 };
}
function params() {
  return new URLSearchParams(period());
}
function showToast(message) {
  $("toast").textContent = message;
  $("toast").hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    $("toast").hidden = true;
  }, 4500);
}
function showView(name) {
  if (!titles[name]) name = "overview";
  for (const [key, title] of Object.entries(titles))
    $("view-" + key).hidden = key !== name;
  document.querySelectorAll(".nav-item").forEach((button) => {
    const active = button.dataset.view === name;
    button.classList.toggle("active", active);
    if (active) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  $("breadcrumb-view").textContent = titles[name][0];
  $("page-title").textContent = titles[name][1];
  $("page-description").textContent = titles[name][2];
  document.title = `${titles[name][0]} · Uptime Monitor`;
  if (location.hash !== `#${name}`) history.replaceState(null, "", `#${name}`);
}
document
  .querySelectorAll("[data-view]")
  .forEach((button) =>
    button.addEventListener("click", () => showView(button.dataset.view)),
  );
window.addEventListener("hashchange", () => showView(location.hash.slice(1)));
showView(location.hash.slice(1));

function setStat(id, value, unit) {
  $(id).replaceChildren(
    document.createTextNode(value),
    ...(unit ? [el("span", unit, "unit")] : []),
  );
}
function renderStats(report) {
  setStat(
    "stat-uptime",
    report.uptime_percent === null ? "—" : report.uptime_percent.toFixed(2),
    report.uptime_percent === null ? "" : "%",
  );
  setStat("stat-outages", String(report.outage_count));
  const total = Math.round(report.downtime_seconds);
  if (total < 60) setStat("stat-downtime", String(total), "s");
  else if (total < 3600)
    $("stat-downtime").replaceChildren(
      document.createTextNode(String(Math.floor(total / 60))),
      el("span", "m", "unit"),
      document.createTextNode(` ${total % 60}`),
      el("span", "s", "unit"),
    );
  else setStat("stat-downtime", duration(total));
  setStat(
    "stat-latency",
    report.average_latency_ms === null
      ? "—"
      : String(Math.round(report.average_latency_ms)),
    report.average_latency_ms === null ? "" : "ms",
  );
  $("stat-threshold").textContent =
    `Interruptions lasting ${report.minimum_outage_seconds}s or longer`;
  $("stat-longest").textContent = report.outage_count
    ? `Longest interruption: ${duration(report.longest_outage_seconds)}`
    : "No recorded outages";
  $("nav-count").textContent = report.outage_count;
}

function renderStatus(status) {
  const sample = status.latest;
  const stale =
    sample &&
    !status.demo &&
    Date.now() / 1000 - sample.checked_at >
      Math.max(30, status.config.interval_seconds * 3);
  const paused = status.paused;
  const state = sample && !stale && !paused ? sample.state : "unknown";
  $("demo-badge").hidden = !status.demo;
  $("demo-notice").hidden = !status.demo;
  $("paused-notice").hidden = !paused;
  $("monitor-badge").replaceChildren(
    el("i"),
    document.createTextNode(
      status.demo
        ? "Example preview"
        : paused
          ? "Monitoring paused"
          : stale
            ? "Check overdue"
            : "Monitoring live",
    ),
  );
  const messages = {
    online: [
      "You’re connected. We’re keeping watch.",
      `Checking every ${status.config.interval_seconds} seconds. Any public target reply confirms connectivity.`,
    ],
    upstream: [
      "Your internet connection is down.",
      "Your router is responding, but none of the public targets replied. The issue may be upstream.",
    ],
    local: [
      "Your local connection needs a look.",
      "Neither your router nor the public internet targets are responding. Check Wi-Fi, cables, and router power.",
    ],
    undetermined: [
      "The internet isn’t responding.",
      "All public targets failed. Router status is unavailable, so the likely location is undetermined.",
    ],
    unknown: [
      paused
        ? "Taking a pause."
        : stale
          ? "Waiting for a fresh check…"
          : status.error
            ? "The connection check needs attention."
            : "Getting to know your connection…",
      paused
        ? "Resume whenever you’re ready. Paused time is not counted as downtime."
        : status.error ||
          "Checking your router and public targets. Unobserved time won’t be counted as an outage.",
    ],
  };
  $("connection-title").textContent = messages[state][0];
  $("connection-description").textContent = messages[state][1];
  $("connection-banner").className =
    `connection-banner${["upstream", "local", "undetermined"].includes(state) ? " problem" : state === "unknown" ? " waiting" : ""}`;
  $("connection-banner")
    .querySelector(".connection-symbol")
    .replaceChildren(icon(state === "online" ? "check" : "pulse"));
  const pauseButton = $("pause-button");
  pauseButton.querySelector("span").textContent = paused
    ? "Resume monitoring"
    : "Pause monitoring";
  pauseButton.disabled = status.demo;
  $("router-address").textContent =
    sample?.router.address || status.gateway || "Auto-detect";
  $("internet-targets").textContent =
    `${status.config.targets.length} independent targets`;
  const routerOk = sample && !paused && !stale ? sample.router.ok : null;
  const internetOk =
    state === "online" ? true : state === "unknown" ? null : false;
  for (const [prefix, ok] of [
    ["router", routerOk],
    ["internet", internetOk],
  ]) {
    $(prefix + "-icon").className =
      `node-icon${ok === false ? " bad" : ok === null ? " wait" : ""}`;
    $(prefix + "-result").className =
      `node-result${ok === false ? " bad" : ok === null ? " wait" : ""}`;
  }
  $("router-result").textContent =
    routerOk === true
      ? `Responding · ${sample.router.latency_ms} ms`
      : routerOk === false
        ? "No reply"
        : paused
          ? "Paused"
          : "Not confirmed";
  $("router-result").title = sample?.router.error || "";
  const replies = sample
    ? sample.targets.filter((t) => t.ok === true).length
    : 0;
  $("internet-result").textContent =
    internetOk === true
      ? `${replies} of ${status.config.targets.length} responding`
      : internetOk === false
        ? "No targets replied"
        : paused
          ? "Paused"
          : "Waiting for checks";
  $("internet-result").title = sample
    ? sample.targets
        .map(
          (t) =>
            `${t.name} (${t.address}): ${t.ok === true ? t.latency_ms + " ms" : t.error || "No reply"}`,
        )
        .join("\n")
    : "";
  $("network-explanation").textContent =
    state === "online"
      ? routerOk === true
        ? "Both sides look healthy. Your local network and the internet are responding."
        : "Internet replies are arriving. Your router may ignore ping requests; check its address in Settings."
      : messages[state][1];
  $("last-checked").textContent = sample
    ? `${status.demo ? "Example check" : "Last check"}: ${timeLabel(sample.checked_at)} · ${status.demo ? "demo data" : "every " + status.config.interval_seconds + "s"}`
    : paused
      ? "Monitoring paused"
      : "Waiting for the first check…";
}

function renderTimeline(report) {
  const svg = svgEl("svg", {
    viewBox: "0 0 1000 32",
    preserveAspectRatio: "none",
    "aria-hidden": "true",
  });
  // Draw short failure markers last, so they remain visible at day/week scale.
  const spans = [...report.spans].sort(
    (a, b) =>
      Number(!["online", "unknown"].includes(a.state)) -
      Number(!["online", "unknown"].includes(b.state)),
  );
  for (const span of spans) {
    const x =
      ((span.started_at - report.start) / (report.end - report.start)) * 1000;
    const width =
      ((span.ended_at - span.started_at) / (report.end - report.start)) * 1000;
    const rect = svgEl("rect", {
      x,
      y: 0,
      width: Math.max(
        ["online", "unknown"].includes(span.state) ? 0.65 : 2,
        width,
      ),
      height: 32,
      fill: colors[span.state] || colors.unknown,
    });
    const title = svgEl("title");
    title.textContent = `${span.state === "online" ? "Online" : span.state === "unknown" ? "Unmonitored" : span.state === "local" ? "Router and internet unreachable" : "Internet unreachable"} · ${timeLabel(span.started_at, true)}–${timeLabel(span.ended_at)} · ${duration(span.ended_at - span.started_at)}`;
    rect.append(title);
    svg.append(rect);
  }
  $("timeline").replaceChildren(svg);
  $("timeline-axis").replaceChildren(
    ...Array.from({ length: 5 }, (_, i) =>
      el(
        "span",
        timeLabel(
          report.start + ((report.end - report.start) * i) / 4,
          $("period").value !== "today",
        ),
      ),
    ),
  );
  $("coverage").textContent =
    `${duration(report.monitored_seconds)} monitored · ${duration(report.unmonitored_seconds)} unmonitored`;
  $("timezone").textContent = Intl.DateTimeFormat().resolvedOptions().timeZone;
  $("timeline-subtitle").textContent = report.spans.length
    ? "Small interruptions. A complete record of what we observed."
    : "Your timeline will build as connection checks arrive.";
}

function renderLatency(report) {
  const width = 600,
    height = 165,
    padding = 28;
  const valid = report.latency.filter((p) => p.latency_ms !== null);
  const max = Math.max(40, ...valid.map((p) => p.latency_ms)) * 1.25;
  const svg = svgEl("svg", {
    viewBox: `0 0 ${width} ${height}`,
    preserveAspectRatio: "none",
    "aria-hidden": "true",
  });
  for (let i = 0; i < 4; i++) {
    const y = 12 + (i * (height - 32)) / 3;
    svg.append(
      svgEl("line", {
        x1: padding,
        x2: width,
        y1: y,
        y2: y,
        stroke: "#e9eee1",
        "stroke-dasharray": "3 5",
        "stroke-width": 1,
      }),
    );
    const label = svgEl("text", {
      x: 0,
      y: y + 3,
      fill: "#a0ac91",
      "font-size": "9",
      "font-family": "Segoe UI, sans-serif",
    });
    label.textContent = Math.round(max * (1 - i / 3));
    svg.append(label);
  }
  let path = "",
    lastBucket = -2;
  for (const point of report.latency) {
    if (point.latency_ms === null) {
      lastBucket = -2;
      continue;
    }
    const x =
      padding +
      ((point.checked_at - report.start) / (report.end - report.start)) *
        (width - padding);
    const y = 12 + (1 - point.latency_ms / max) * (height - 32);
    path += `${point.bucket === lastBucket + 1 ? "L" : "M"}${x.toFixed(2)},${y.toFixed(2)} `;
    lastBucket = point.bucket;
    if (valid.length < 3)
      svg.append(svgEl("circle", { cx: x, cy: y, r: 2.5, fill: "#87ad70" }));
  }
  svg.append(
    svgEl("path", {
      d: path,
      stroke: "#87ad70",
      "stroke-width": "2",
      fill: "none",
      "stroke-linejoin": "round",
      "vector-effect": "non-scaling-stroke",
    }),
  );
  if (!valid.length) {
    const label = svgEl("text", {
      x: 320,
      y: 88,
      "text-anchor": "middle",
      fill: "#99a38c",
      "font-size": 12,
      "font-family": "Segoe UI, sans-serif",
    });
    label.textContent = "Your first replies will appear here.";
    svg.append(label);
  }
  $("latency-chart").replaceChildren(svg);
  $("latency-range").textContent = valid.length
    ? `${Math.round(Math.min(...valid.map((p) => p.latency_ms)))}–${Math.round(Math.max(...valid.map((p) => p.latency_ms)))} ms · ${report.check_count.toLocaleString("en-US")} checks`
    : "No replies in this period";
}

function outageTable(events, container, emptyFiltered = false) {
  if (!events.length) {
    const empty = el("div", undefined, "empty-state"),
      circle = el("div", undefined, "empty-icon");
    circle.append(icon("check"));
    empty.append(
      circle,
      el(
        "h3",
        emptyFiltered
          ? "No outages match this filter."
          : "No outages on the record.",
      ),
      el(
        "p",
        emptyFiltered
          ? "Try another cause or a different period."
          : "We’ll record an interruption here when one is observed. Keep the monitor running to build your connection history.",
      ),
    );
    container.replaceChildren(empty);
    return;
  }
  const table = el("table"),
    head = el("thead"),
    headRow = el("tr"),
    body = el("tbody");
  for (const label of [
    "WHEN IT STARTED",
    "DURATION",
    "LIKELY LOCATION",
    "ROUTER EVIDENCE",
    "STATUS",
  ])
    headRow.append(el("th", label));
  head.append(headRow);
  for (const event of events) {
    const row = el("tr"),
      when = el("td"),
      cause = el("td"),
      evidence = el("td"),
      status = el("td");
    when.append(
      el("strong", timeLabel(event.started_at)),
      el(
        "small",
        new Date(event.started_at * 1000).toLocaleDateString("en-GB", {
          day: "numeric",
          month: "short",
        }),
      ),
    );
    const badge = el(
      "span",
      undefined,
      `cause${event.cause === "local" ? " local" : event.cause === "upstream" ? "" : " other"}`,
    );
    badge.append(
      el("i"),
      document.createTextNode(
        event.cause === "upstream"
          ? "Internet issue"
          : event.cause === "local"
            ? "Local network"
            : event.cause === "mixed"
              ? "Mixed evidence"
              : "Undetermined",
      ),
    );
    cause.append(badge);
    evidence.textContent =
      event.cause === "upstream"
        ? "Responded"
        : event.cause === "local"
          ? "No reply"
          : event.cause === "mixed"
            ? "Changed during outage"
            : "Unknown";
    evidence.title = event.explanation;
    status.append(
      el(
        "span",
        event.status === "recovered"
          ? "↗ Recovered"
          : event.status === "ongoing"
            ? "● Ongoing"
            : "Observation ended",
        `status-label ${event.status}`,
      ),
    );
    status.title =
      event.status === "interrupted"
        ? "Monitoring stopped before recovery was confirmed."
        : event.ended_at
          ? `Ended ${timeLabel(event.ended_at, true)}`
          : "Still being observed";
    row.append(
      when,
      el("td", duration(event.duration_seconds)),
      cause,
      evidence,
      status,
    );
    body.append(row);
  }
  table.append(head, body);
  const wrap = el("div", undefined, "table-wrap");
  wrap.append(table);
  container.replaceChildren(wrap);
}
function renderHistory(report) {
  outageTable(report.outages.slice(0, 4), $("recent-outages"));
  const filter = $("cause-filter").value;
  const events = report.outages.filter(
    (e) =>
      filter === "all" ||
      (filter === "other"
        ? !["upstream", "local"].includes(e.cause)
        : e.cause === filter),
  );
  outageTable(events, $("history-outages"), filter !== "all");
  $("history-summary").textContent =
    `${report.outage_count} interruptions · ${duration(report.downtime_seconds)} of downtime${report.outages_truncated ? ". Showing the latest 500; export for the complete list." : "."}`;
}
function renderReport(report) {
  $("report-headline").textContent =
    `${report.outage_count} ${report.outage_count === 1 ? "outage" : "outages"}. ${duration(report.downtime_seconds)} of downtime.`;
  $("report-period").textContent =
    `${timeLabel(report.start, true)} — ${timeLabel(report.end, true)} · ${Intl.DateTimeFormat().resolvedOptions().timeZone}`;
  const fields = [
    ["Monitored time", duration(report.monitored_seconds)],
    ["Unmonitored time", duration(report.unmonitored_seconds)],
    [
      "Connection availability",
      report.uptime_percent === null
        ? "No monitored time"
        : report.uptime_percent.toFixed(3) + "%",
    ],
    ["Longest interruption", duration(report.longest_outage_seconds)],
    ["Minimum outage length", `${report.minimum_outage_seconds}s`],
  ];
  $("report-details").replaceChildren(
    ...fields.map(([name, value]) => {
      const row = el("div");
      row.append(el("dt", name), el("dd", value));
      return row;
    }),
  );
}
function renderSettings(status) {
  if (settingsLoaded) return;
  const config = status.config;
  $("interval").value = config.interval_seconds;
  $("timeout").value = config.timeout_ms;
  $("threshold").value = config.minimum_outage_seconds;
  $("router").value = config.router;
  $("automatic-updates").checked = config.check_updates_automatically !== false;
  $("target-fields").replaceChildren(
    ...config.targets.map((target, index) => {
      const row = el("div", undefined, "target-row");
      for (const key of ["name", "address"]) {
        const label = el(
          "label",
          key === "name" ? `Target ${index + 1} name` : "IPv4 address",
        );
        const input = el("input");
        input.value = target[key];
        input.required = true;
        input.dataset.key = key;
        input.maxLength = key === "name" ? 40 : 15;
        label.append(input);
        row.append(label);
      }
      return row;
    }),
  );
  if (status.demo)
    $("settings-form")
      .querySelectorAll("input,button")
      .forEach((input) => {
        input.disabled = true;
      });
  settingsLoaded = true;
}

function renderUpdates(update, demo) {
  if (!update) return;
  $("installed-version").textContent = update.current_version;
  $("update-notice").hidden = update.phase !== "available" || demo;
  $("update-notice-text").textContent =
    `Version ${update.latest_version || "latest"} is available. `;
  updateRestarting = update.phase === "restarting";
  const busy = ["checking", "downloading", "restarting"].includes(update.phase);
  $("check-updates").disabled = busy;
  $("check-updates").textContent =
    update.phase === "checking" ? "Checking GitHub…" : "Check for updates";
  $("install-update").hidden =
    update.phase !== "available" || !update.install_supported || demo;
  $("install-update").disabled = busy;
  $("install-update").textContent =
    `Update to ${update.latest_version || "latest"} & restart`;
  $("update-release-link").hidden = !update.release_url;
  if (update.release_url) $("update-release-link").href = update.release_url;
  const progress = update.total_bytes
    ? Math.min(
        100,
        Math.round((update.downloaded_bytes / update.total_bytes) * 100),
      )
    : 0;
  const messages = {
    idle: "Check GitHub for the latest stable version.",
    checking:
      "Checking this project’s GitHub releases. Monitoring continues in the background.",
    up_to_date: "You’re up to date. This is the latest stable version.",
    available: `Version ${update.latest_version} is available.${demo ? " Start the live monitor to install it." : update.install_supported ? " Download it and restart without losing your history." : " Download the release for your platform from GitHub."}`,
    downloading: `Downloading update… ${progress}%. Your connection is still being monitored.`,
    restarting:
      "Update verified. Restarting the monitor… This dashboard will reconnect automatically.",
    error:
      update.error ||
      "The update check failed. Try again when the internet is available.",
  };
  $("update-description").textContent = messages[update.phase] || messages.idle;
  $("update-badge").textContent =
    update.phase === "available"
      ? "UPDATE AVAILABLE"
      : busy
        ? "IN PROGRESS"
        : "GITHUB RELEASES";
  $("update-checked").textContent = update.checked_at
    ? `Last checked: ${timeLabel(update.checked_at, true)}`
    : "";
  if (expectedUpdate && update.current_version === expectedUpdate) {
    showToast(
      `Updated to version ${update.current_version}. Your monitoring history is preserved.`,
    );
    expectedUpdate = null;
  }
}

async function refresh() {
  if (loading) {
    refreshRequested = true;
    return;
  }
  loading = true;
  const selectedPeriod = $("period").value;
  try {
    const response = await fetch(`/api/dashboard?${params()}`, {
      signal: AbortSignal.timeout(10000),
    });
    if (!response.ok) throw new Error("Could not load the local dashboard.");
    const result = await response.json();
    if (selectedPeriod !== $("period").value) {
      refreshRequested = true;
      return;
    }
    data = result;
    $("connection-error").hidden = true;
    renderStats(data.report);
    renderStatus(data.status);
    renderTimeline(data.report);
    renderLatency(data.report);
    renderHistory(data.report);
    renderReport(data.report);
    renderSettings(data.status);
    renderUpdates(data.updates, data.status.demo);
  } catch (error) {
    if (updateRestarting) {
      $("connection-error").textContent =
        "Installing the verified update. The dashboard will reconnect when the monitor restarts.";
      $("connection-error").hidden = false;
      $("monitor-badge").replaceChildren(
        el("i"),
        document.createTextNode("Restarting for update"),
      );
      return;
    }
    $("connection-error").textContent =
      "The local monitor is unavailable. Displayed results may be out of date. Start the monitor again to resume recording; this gap will remain unmonitored.";
    $("connection-error").hidden = false;
    $("monitor-badge").replaceChildren(
      el("i"),
      document.createTextNode("Monitor unavailable"),
    );
    $("connection-title").textContent = "The local monitor is unavailable.";
    $("connection-description").textContent =
      "Start the monitor again to resume checks. We can’t confirm your connection right now.";
    $("connection-banner").className = "connection-banner waiting";
    $("connection-banner")
      .querySelector(".connection-symbol")
      .replaceChildren(icon("pulse"));
    $("pause-button").disabled = true;
  } finally {
    loading = false;
    if (refreshRequested) {
      refreshRequested = false;
      refresh();
    }
  }
}
async function post(path, body) {
  const response = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  const result = await response.json();
  if (!response.ok)
    throw new Error(result.error || "Could not save this change.");
}
async function control(action) {
  $("pause-button").disabled = true;
  try {
    await post("/api/control", { action });
    await refresh();
  } catch (error) {
    showToast(error.message);
    $("pause-button").disabled = false;
  }
}
$("pause-button").addEventListener("click", () =>
  control(data?.status.paused ? "resume" : "pause"),
);
$("resume-inline").addEventListener("click", () => control("resume"));
$("period").addEventListener("change", refresh);
$("cause-filter").addEventListener("change", () => {
  if (data) renderHistory(data.report);
});
$("settings-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = $("save-settings");
  button.disabled = true;
  const config = {
    interval_seconds: Number($("interval").value),
    timeout_ms: Number($("timeout").value),
    minimum_outage_seconds: Number($("threshold").value),
    router: $("router").value.trim(),
    check_updates_automatically: $("automatic-updates").checked,
    targets: [...document.querySelectorAll(".target-row")].map((row) =>
      Object.fromEntries(
        [...row.querySelectorAll("input")].map((input) => [
          input.dataset.key,
          input.value.trim(),
        ]),
      ),
    ),
  };
  try {
    await post("/api/settings", config);
    $("settings-message").hidden = true;
    settingsLoaded = false;
    showToast("Preferences saved. The next check will use your settings.");
    await refresh();
  } catch (error) {
    $("settings-message").textContent = error.message;
    $("settings-message").hidden = false;
    $("settings-message").className = "notice error";
  } finally {
    button.disabled = false;
  }
});
function download(format) {
  const anchor = el("a");
  anchor.href = `/api/export?${params()}&format=${format}`;
  anchor.download = "";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
}
for (const id of ["export-top", "download-txt"])
  $(id).addEventListener("click", () => download("txt"));
$("download-csv").addEventListener("click", () => download("csv"));
$("download-json").addEventListener("click", () => download("json"));
$("check-updates").addEventListener("click", async () => {
  $("check-updates").disabled = true;
  try {
    await post("/api/updates/check", {});
    await refresh();
  } catch (error) {
    showToast(error.message);
    $("check-updates").disabled = false;
  }
});
$("install-update").addEventListener("click", async () => {
  $("install-update").disabled = true;
  expectedUpdate = data?.updates?.latest_version || null;
  try {
    await post("/api/updates/install", {});
    await refresh();
  } catch (error) {
    expectedUpdate = null;
    showToast(error.message);
    $("install-update").disabled = false;
  }
});
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refresh();
});
refresh();
setInterval(() => {
  if (!document.hidden) refresh();
}, 3000);
