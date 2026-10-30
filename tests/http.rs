use internet_uptime_monitor::{monitor::App, platform, server, storage::Store};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};

struct Service {
    app: Arc<App>,
    port: u16,
    threads: Vec<JoinHandle<()>>,
}
impl Service {
    fn new() -> Self {
        let app = App::new(Store::memory().unwrap(), false).unwrap();
        let listener = server::bind(0).unwrap();
        let port = listener.server_addr().to_ip().unwrap().port();
        let threads = server::start(listener, &app);
        Self { app, port, threads }
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &str,
        host: Option<&str>,
        origin: Option<&str>,
        content_type: &str,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream,"{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{}\r\n{body}",host.map(str::to_string).unwrap_or_else(||format!("127.0.0.1:{}",self.port)),body.len(),origin.map(|o|format!("Origin: {o}\r\n")).unwrap_or_default()).unwrap();
        let mut result = String::new();
        stream.read_to_string(&mut result).unwrap();
        let (headers, body) = result.split_once("\r\n\r\n").unwrap();
        let code = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (code, body.into())
    }
    fn get(&self, path: &str) -> (u16, String) {
        self.request("GET", path, "", None, None, "application/json")
    }
    fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.request("POST", path, body, None, None, "application/json")
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.app.stop();
        for thread in self.threads.drain(..) {
            thread.join().unwrap();
        }
    }
}

#[test]
fn assets_are_embedded_and_dashboard_is_available_offline() {
    let service = Service::new();
    for path in [
        "/",
        "/app.js",
        "/style.css",
        "/icon.svg",
        "/api/dashboard",
        "/api/health",
        "/api/updates",
    ] {
        assert_eq!(service.get(path).0, 200, "{path}");
    }
    assert!(platform::monitor_running(service.port));
    let data: Value = serde_json::from_str(&service.get("/api/dashboard").1).unwrap();
    assert!(data["report"]["uptime_percent"].is_null());
    assert_eq!(data["report"]["outage_count"], 0);
    assert_eq!(
        data["updates"]["current_version"],
        env!("CARGO_PKG_VERSION")
    );
}
#[test]
fn exports_and_controls_use_the_same_persistent_evidence() {
    let service = Service::new();
    for format in ["txt", "csv", "json"] {
        let (code, body) = service.get(&format!("/api/export?format={format}"));
        assert_eq!(code, 200);
        assert!(!body.is_empty());
    }
    assert_eq!(service.post("/api/control", r#"{"action":"pause"}"#).0, 200);
    assert!(service.app.snapshot().paused);
    assert_eq!(
        service.post("/api/control", r#"{"action":"resume"}"#).0,
        200
    );
    assert!(!service.app.snapshot().paused);
    assert_eq!(
        service
            .post("/api/settings", r#"{"interval_seconds":10}"#)
            .0,
        200
    );
    assert_eq!(
        service
            .app
            .store
            .lock()
            .unwrap()
            .config()
            .unwrap()
            .interval_seconds,
        10
    );
}
#[test]
fn rejects_invalid_inputs_cross_origin_writes_and_path_traversal() {
    let service = Service::new();
    for query in ["start=NaN", "end=NaN", "start=1&end=0", "start=-inf"] {
        assert_eq!(service.get(&format!("/api/dashboard?{query}")).0, 400);
    }
    for body in [
        "[]",
        "{",
        r#"{"interval_seconds":0}"#,
        r#"{"router":"127.0.0.1"}"#,
    ] {
        assert_eq!(service.post("/api/settings", body).0, 400);
    }
    assert_eq!(
        service
            .request(
                "GET",
                "/api/dashboard",
                "",
                Some("attacker.example"),
                None,
                "application/json"
            )
            .0,
        403
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/control",
                r#"{"action":"pause"}"#,
                None,
                Some("https://attacker.example"),
                "application/json"
            )
            .0,
        403
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/control",
                r#"{"action":"pause"}"#,
                None,
                None,
                "text/plain"
            )
            .0,
        415
    );
    assert_eq!(service.get("/../../LICENSE").0, 404);
    assert_eq!(service.get("/api/export?format=exe").0, 400);
}
#[test]
fn shutdown_closes_the_local_service_gracefully() {
    let service = Service::new();
    assert_eq!(
        platform::local_request(service.port, "POST", "/api/shutdown").unwrap()["ok"],
        true
    );
    assert!(service.app.is_stopping());
}

#[test]
fn update_install_requires_a_checked_release_and_same_origin() {
    let service = Service::new();
    assert_eq!(service.post("/api/updates/install", "{}").0, 400);
    assert!(!service.app.is_stopping());
    assert_eq!(
        service
            .request(
                "POST",
                "/api/updates/check",
                "{}",
                None,
                Some("https://attacker.example"),
                "application/json"
            )
            .0,
        403
    );
    assert_eq!(
        service
            .post("/api/settings", r#"{"check_updates_automatically":false}"#)
            .0,
        200
    );
    assert!(!service.app.snapshot().config.check_updates_automatically);
}
