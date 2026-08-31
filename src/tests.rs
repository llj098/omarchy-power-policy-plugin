use std::{
    io::{BufRead, BufReader, ErrorKind, Read},
    os::unix::net::UnixStream,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::time::{Instant, sleep};
use zbus::{Connection, connection::Builder, zvariant::OwnedFd};

use super::{
    LOGIND_NAME, LOGIND_PATH, PolicyEngine, UPOWER_NAME, UPOWER_PATH, read_state,
    relevant_properties, run_daemon_on_connection,
};

struct PrivateBus {
    child: Child,
    _stdout: BufReader<ChildStdout>,
    address: String,
}

impl PrivateBus {
    fn start() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private dbus-daemon");
        let mut stdout = BufReader::new(child.stdout.take().expect("capture D-Bus address"));
        let mut address = String::new();
        stdout
            .read_line(&mut address)
            .expect("read private D-Bus address");
        assert!(!address.trim().is_empty(), "private D-Bus address is empty");

        Self {
            child,
            _stdout: stdout,
            address: address.trim().to_owned(),
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct MockLogindState {
    inhibit_calls: AtomicUsize,
    suspend_calls: AtomicUsize,
    inhibitor_peer: Mutex<Option<UnixStream>>,
    other_inhibitor_peers: Mutex<Vec<UnixStream>>,
}

impl MockLogindState {
    fn inhibitor_is_open(&self) -> bool {
        let mut guard = self.inhibitor_peer.lock().expect("lock inhibitor peer");
        let peer = guard.as_mut().expect("inhibitor was not acquired");
        let mut byte = [0_u8; 1];
        match peer.read(&mut byte) {
            Ok(0) => false,
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => true,
            Err(error) => panic!("inspect inhibitor peer: {error}"),
        }
    }
}

struct MockLogind {
    state: Arc<MockLogindState>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl MockLogind {
    fn inhibit(
        &self,
        what: &str,
        _who: &str,
        _why: &str,
        _mode: &str,
    ) -> zbus::fdo::Result<OwnedFd> {
        let (returned, peer) =
            UnixStream::pair().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        peer.set_nonblocking(true)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        if what == "handle-lid-switch" {
            *self
                .state
                .inhibitor_peer
                .lock()
                .expect("lock inhibitor peer") = Some(peer);
            self.state.inhibit_calls.fetch_add(1, Ordering::SeqCst);
        } else {
            self.state
                .other_inhibitor_peers
                .lock()
                .expect("lock other inhibitor peers")
                .push(peer);
        }
        let returned: std::os::fd::OwnedFd = returned.into();
        Ok(returned.into())
    }

    fn suspend(&self, _interactive: bool) {
        self.state.suspend_calls.fetch_add(1, Ordering::SeqCst);
    }
}

struct ActualUpower {
    child: Child,
}

impl ActualUpower {
    fn start(address: &str) -> Self {
        let child = Command::new("/usr/lib/upowerd")
            .arg("--verbose")
            .env("DBUS_SYSTEM_BUS_ADDRESS", address)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start actual upowerd on private D-Bus");
        Self { child }
    }

    fn stop(mut self) {
        self.terminate();
    }

    fn terminate(&mut self) {
        if self
            .child
            .try_wait()
            .expect("inspect actual upowerd")
            .is_none()
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for ActualUpower {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Default)]
struct MockUpowerState {
    lid_reads: AtomicUsize,
}

struct MockUpower {
    state: Arc<MockUpowerState>,
}

#[zbus::interface(name = "org.freedesktop.UPower")]
impl MockUpower {
    #[zbus(property)]
    fn on_battery(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn lid_is_closed(&self) -> bool {
        self.state.lid_reads.fetch_add(1, Ordering::SeqCst);
        true
    }
}

async fn serve_logind(address: &str, state: Arc<MockLogindState>) -> Connection {
    Builder::address(address)
        .expect("use private D-Bus address")
        .name(LOGIND_NAME)
        .expect("request mock logind name")
        .serve_at(LOGIND_PATH, MockLogind { state })
        .expect("serve mock logind")
        .build()
        .await
        .expect("connect mock logind")
}

async fn serve_upower(address: &str, state: Arc<MockUpowerState>) -> Connection {
    Builder::address(address)
        .expect("use private D-Bus address")
        .name(UPOWER_NAME)
        .expect("request mock UPower name")
        .serve_at(UPOWER_PATH, MockUpower { state })
        .expect("serve mock UPower")
        .build()
        .await
        .expect("connect mock UPower")
}

async fn connect_client(address: &str) -> Connection {
    Builder::address(address)
        .expect("use private D-Bus address")
        .build()
        .await
        .expect("connect policy daemon")
}

async fn wait_for_actual_upower(connection: &Connection) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(upower) = zbus::Proxy::new(
            connection,
            UPOWER_NAME,
            UPOWER_PATH,
            "org.freedesktop.UPower",
        )
        .await
        {
            if read_state(&upower).await.is_ok() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for actual upowerd"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_until(message: &str, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out: {message}");
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn upower_restart_keeps_daemon_inhibitor() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = PrivateBus::start();
        let logind_state = Arc::new(MockLogindState::default());
        let upower_state = Arc::new(MockUpowerState::default());
        let _logind = serve_logind(&bus.address, Arc::clone(&logind_state)).await;
        let first_upower = serve_upower(&bus.address, Arc::clone(&upower_state)).await;
        let client = connect_client(&bus.address).await;

        let daemon = tokio::spawn(run_daemon_on_connection(client, true));
        wait_until("initial inhibitor", || {
            logind_state.inhibit_calls.load(Ordering::SeqCst) == 1
        })
        .await;
        wait_until("initial UPower state read", || {
            upower_state.lid_reads.load(Ordering::SeqCst) >= 1
        })
        .await;
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        let reads_before_restart = upower_state.lid_reads.load(Ordering::SeqCst);
        first_upower
            .release_name(UPOWER_NAME)
            .await
            .expect("release first UPower owner");
        drop(first_upower);

        // The old bug dropped the inhibitor immediately on this owner change,
        // then waited before reconnecting. Check inside that former gap.
        sleep(Duration::from_millis(200)).await;
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.inhibit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        let _second_upower = serve_upower(&bus.address, Arc::clone(&upower_state)).await;
        wait_until("UPower state read after restart", || {
            upower_state.lid_reads.load(Ordering::SeqCst) > reads_before_restart
        })
        .await;

        assert!(!daemon.is_finished());
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.inhibit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        daemon.abort();
        let _ = daemon.await;
        wait_until("inhibitor release after daemon exit", || {
            !logind_state.inhibitor_is_open()
        })
        .await;
    })
    .await
    .expect("isolated D-Bus restart test timed out");
}

#[tokio::test(flavor = "current_thread")]
async fn actual_upower_process_restart_keeps_daemon_inhibitor() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let bus = PrivateBus::start();
        let logind_state = Arc::new(MockLogindState::default());
        let _logind = serve_logind(&bus.address, Arc::clone(&logind_state)).await;
        let control = connect_client(&bus.address).await;

        let first_upower = ActualUpower::start(&bus.address);
        wait_for_actual_upower(&control).await;
        let client = connect_client(&bus.address).await;
        let daemon = tokio::spawn(run_daemon_on_connection(client, true));

        wait_until("policy inhibitor with actual upowerd", || {
            logind_state.inhibit_calls.load(Ordering::SeqCst) == 1
        })
        .await;
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        first_upower.stop();
        sleep(Duration::from_millis(200)).await;
        assert!(!daemon.is_finished());
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.inhibit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        let _second_upower = ActualUpower::start(&bus.address);
        wait_for_actual_upower(&control).await;
        sleep(Duration::from_millis(2200)).await;

        assert!(!daemon.is_finished());
        assert!(logind_state.inhibitor_is_open());
        assert_eq!(logind_state.inhibit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(logind_state.suspend_calls.load(Ordering::SeqCst), 0);

        daemon.abort();
        let _ = daemon.await;
        wait_until("inhibitor release after actual UPower test", || {
            !logind_state.inhibitor_is_open()
        })
        .await;
    })
    .await
    .expect("actual upowerd restart test timed out");
}

#[test]
fn close_on_ac_then_unplug_suspends_once() {
    let mut policy = PolicyEngine::default();
    assert!(!policy.update(false, false));
    assert!(!policy.update(false, true));
    assert!(policy.update(true, true));
    assert!(!policy.update(true, true));
}

#[test]
fn close_while_on_battery_suspends() {
    let mut policy = PolicyEngine::default();
    assert!(!policy.update(true, false));
    assert!(policy.update(true, true));
}

#[test]
fn opening_lid_rearms_policy() {
    let mut policy = PolicyEngine::default();
    assert!(policy.update(true, true));
    assert!(!policy.update(true, false));
    assert!(policy.update(true, true));
}

#[test]
fn plugging_ac_rearms_closed_lid() {
    let mut policy = PolicyEngine::default();
    assert!(policy.update(true, true));
    assert!(!policy.update(false, true));
    assert!(policy.update(true, true));
}

#[test]
fn open_lid_unplug_does_nothing() {
    let mut policy = PolicyEngine::default();
    assert!(!policy.update(false, false));
    assert!(!policy.update(true, false));
}

#[test]
fn filters_relevant_property_names() {
    let changed = ["DaemonVersion", "OnBattery", "LidIsClosed"];
    let invalidated = ["LidIsClosed"];
    assert_eq!(
        relevant_properties(changed.into_iter(), invalidated.into_iter()),
        ["LidIsClosed", "OnBattery"]
    );
}
