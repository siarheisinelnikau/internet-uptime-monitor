use internet_uptime_monitor::instance;
use std::{
    net::TcpListener,
    process::{Child, Command},
    time::{Duration, Instant},
};
fn command(data: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_internet-uptime-monitor"));
    command.arg("--data-dir").arg(data);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
struct Running {
    child: Child,
    data: tempfile::TempDir,
}
impl Running {
    fn start(extra: &[String]) -> Self {
        let data = tempfile::tempdir().unwrap();
        let child = command(data.path())
            .args(["--headless", "--start-paused"])
            .args(extra)
            .spawn()
            .unwrap();
        let running = Self { child, data };
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if instance::read(running.data.path(), false).is_some_and(|s| s.ready) {
                return running;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!("Native instance did not become ready");
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = instance::send(self.data.path(), false, "stop");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn occupied_ports_cannot_prevent_native_startup() {
    let occupied = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    occupied.set_nonblocking(true).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let running = Running::start(&["--port".into(), port.to_string()]);
    let session = instance::read(running.data.path(), false).unwrap();
    assert_eq!(session.pid, running.child.id());
    assert!(session.ready);
    assert!(matches!(
        occupied.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}
#[test]
fn duplicate_launch_exits_successfully_and_keeps_the_first_instance() {
    let mut running = Running::start(&[]);
    let first = instance::read(running.data.path(), false).unwrap();
    assert!(
        command(running.data.path())
            .arg("--headless")
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        instance::read(running.data.path(), false).unwrap().pid,
        first.pid
    );
    assert!(running.child.try_wait().unwrap().is_none());
}
#[test]
fn stop_uses_data_directory_ipc_instead_of_a_port() {
    let mut running = Running::start(&[]);
    assert!(
        command(running.data.path())
            .arg("--stop")
            .status()
            .unwrap()
            .success()
    );
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if let Some(status) = running.child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("IPC shutdown did not complete");
}
#[test]
fn stopping_an_inactive_directory_does_not_touch_another_instance() {
    let mut running = Running::start(&[]);
    let other = tempfile::tempdir().unwrap();
    assert!(
        command(other.path())
            .arg("--stop")
            .status()
            .unwrap()
            .success()
    );
    assert!(running.child.try_wait().unwrap().is_none());
}
