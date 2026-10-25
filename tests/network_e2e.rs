//! Opt-in, privileged network integration tests using only private namespaces.
//!
//! Run explicitly with `LIVE777_NETWORK_E2E=1 cargo nextest run --features
//! rsmpeg -E 'binary(network_e2e)' --run-ignored ignored-only` as root with
//! CAP_NET_ADMIN. Every test is ignored by default: setup creates network
//! namespaces and veth pairs. No default route, host firewall, host loopback,
//! or public ICE server is used.

#![cfg(all(target_os = "linux", feature = "rsmpeg"))]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

const SERVER: &str = "10.77.0.1";
const CLIENT: &str = "10.77.0.2";
const TURN_CLIENT: &str = "10.77.1.1";
const CLIENT_TURN: &str = "10.77.1.2";
const SERVER_TURN: &str = "10.77.2.1";
const TURN_SERVER: &str = "10.77.2.2";
const HTTP_PORT: u16 = 7777;
const TURN_PORT: u16 = 3478;
const ICE_ADDRS_ENV: &str = "LIVE777_WEBRTC_ICE_UDP_ADDRS";

static NEXT_TOPOLOGY: AtomicU64 = AtomicU64::new(0);

macro_rules! bin_path {
    ($name:literal) => {
        option_env!(concat!("CARGO_BIN_EXE_", $name)).map_or_else(
            || {
                let mut path = std::env::current_exe().unwrap();
                path.pop();
                path.pop();
                path.push($name);
                path
            },
            PathBuf::from,
        )
    };
}

#[derive(Clone, Copy)]
enum Namespace {
    Client,
    Server,
    Turn,
}

impl Namespace {
    fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
            Self::Turn => "turn",
        }
    }
}

struct Topology {
    names: BTreeMap<&'static str, String>,
    interfaces: BTreeMap<&'static str, String>,
    owned_root_interfaces: Vec<String>,
    tempdir: TempDir,
}

impl Topology {
    fn create() -> Self {
        preflight();
        let id = NEXT_TOPOLOGY.fetch_add(1, Ordering::Relaxed);
        let suffix = format!("{}x{id}", std::process::id());
        let tempdir = tempfile::Builder::new()
            .prefix("live777-network-e2e-")
            .tempdir()
            .expect("create network e2e temporary directory");
        let mut topology = Self {
            names: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            owned_root_interfaces: Vec::new(),
            tempdir,
        };
        for namespace in [Namespace::Client, Namespace::Server, Namespace::Turn] {
            let name = format!("l77e2e-{suffix}-{}", namespace.as_str());
            run_checked(Command::new("ip").args(["netns", "add", &name]));
            topology.names.insert(namespace.as_str(), name);
        }
        for link in ["cs", "sc", "ct", "tc", "st", "ts"] {
            topology.interfaces.insert(link, format!("{link}{suffix}"));
        }

        topology.veth(Namespace::Client, Namespace::Server, "cs", "sc");
        topology.veth(Namespace::Client, Namespace::Turn, "ct", "tc");
        topology.veth(Namespace::Server, Namespace::Turn, "st", "ts");
        for namespace in [Namespace::Client, Namespace::Server, Namespace::Turn] {
            topology.ip(namespace, &["link", "set", "lo", "up"]);
        }
        topology.address(Namespace::Client, "cs", &format!("{CLIENT}/30"));
        topology.address(Namespace::Server, "sc", &format!("{SERVER}/30"));
        topology.address(Namespace::Client, "ct", &format!("{CLIENT_TURN}/30"));
        topology.address(Namespace::Turn, "tc", &format!("{TURN_CLIENT}/30"));
        topology.address(Namespace::Server, "st", &format!("{SERVER_TURN}/30"));
        topology.address(Namespace::Turn, "ts", &format!("{TURN_SERVER}/30"));
        let turn_client_interface = topology.interface(Namespace::Turn, "tc").to_owned();
        let turn_server_interface = topology.interface(Namespace::Turn, "ts").to_owned();
        // CLIENT is the STUN source/srflx address, so coturn returns Binding
        // responses over the client-turn link. Relayed media instead targets
        // SERVER over the server-turn link. Both destinations are local to the
        // receiving namespace, so these /32 routes require no IP forwarding.
        topology.ip(
            Namespace::Turn,
            &[
                "route",
                "add",
                &format!("{CLIENT}/32"),
                "via",
                CLIENT_TURN,
                "dev",
                &turn_client_interface,
            ],
        );
        topology.ip(
            Namespace::Turn,
            &[
                "route",
                "add",
                &format!("{SERVER}/32"),
                "via",
                SERVER_TURN,
                "dev",
                &turn_server_interface,
            ],
        );
        topology
    }

    fn name(&self, namespace: Namespace) -> &str {
        &self.names[namespace.as_str()]
    }

    fn interface(&self, namespace: Namespace, link: &str) -> &str {
        let _ = self.name(namespace);
        &self.interfaces[link]
    }

    fn veth(&mut self, left: Namespace, right: Namespace, left_link: &str, right_link: &str) {
        let left_if = self.interface(left, left_link).to_owned();
        let right_if = self.interface(right, right_link).to_owned();
        run_checked(Command::new("ip").args([
            "link", "add", &left_if, "type", "veth", "peer", "name", &right_if,
        ]));
        self.owned_root_interfaces
            .extend([left_if.clone(), right_if.clone()]);
        run_checked(Command::new("ip").args(["link", "set", &left_if, "netns", self.name(left)]));
        run_checked(Command::new("ip").args(["link", "set", &right_if, "netns", self.name(right)]));
    }

    fn address(&self, namespace: Namespace, link: &str, address: &str) {
        let interface = self.interface(namespace, link).to_owned();
        self.ip(namespace, &["address", "add", address, "dev", &interface]);
        self.ip(namespace, &["link", "set", &interface, "up"]);
    }

    fn ip(&self, namespace: Namespace, args: &[&str]) -> Output {
        let mut command = Command::new("ip");
        command.args(["netns", "exec", self.name(namespace), "ip"]);
        command.args(args);
        run_checked(&mut command)
    }

    fn command(&self, namespace: Namespace, program: &Path, ice_addr: Option<&str>) -> Command {
        let mut command = Command::new("ip");
        command.args(["netns", "exec", self.name(namespace)]);
        command.arg(program);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.env("NO_PROXY", "*").env("no_proxy", "*");
        for variable in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "LIVE777_WEBRTC_ICE_UDP_ADDR",
            "LIVETWO_WEBRTC_ICE_UDP_ADDR",
        ] {
            command.env_remove(variable);
        }
        if let Some(address) = ice_addr {
            command.env(ICE_ADDRS_ENV, format!("{address}:0"));
        } else {
            command.env_remove(ICE_ADDRS_ENV);
        }
        command
    }

    fn set_client_server_link(&self, up: bool) {
        let interface = self.interface(Namespace::Client, "cs").to_owned();
        self.ip(
            Namespace::Client,
            &["link", "set", &interface, if up { "up" } else { "down" }],
        );
    }

    fn apply_netem(&self) {
        let interface = self.interface(Namespace::Client, "cs").to_owned();
        run_checked(Command::new("ip").args([
            "netns",
            "exec",
            self.name(Namespace::Client),
            "tc",
            "qdisc",
            "add",
            "dev",
            &interface,
            "root",
            "netem",
            "loss",
            "5%",
            "delay",
            "80ms",
            "20ms",
        ]));
    }
}

impl Drop for Topology {
    fn drop(&mut self) {
        for name in self.names.values() {
            let output = Command::new("ip").args(["netns", "delete", name]).output();
            match output {
                Ok(output) if output.status.success() => {}
                Ok(output) => eprintln!(
                    "network e2e cleanup could not delete namespace {name}: status={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => {
                    eprintln!("network e2e cleanup could not run ip netns delete {name}: {error}")
                }
            }
        }
        for interface in &self.owned_root_interfaces {
            let _ = Command::new("ip")
                .args(["link", "delete", interface])
                .output();
        }
    }
}

struct ChildGuard {
    label: String,
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    drains: Vec<JoinHandle<()>>,
}

impl ChildGuard {
    fn spawn(label: impl Into<String>, command: &mut Command) -> Self {
        let label = label.into();
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("spawn {label}: {error}; command: {command:?}"));
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let drains = vec![
            drain(child.stdout.take().unwrap(), Arc::clone(&stdout)),
            drain(child.stderr.take().unwrap(), Arc::clone(&stderr)),
        ];
        Self {
            label,
            child,
            stdout,
            stderr,
            drains,
        }
    }

    fn output(&self) -> (String, String) {
        (
            String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned(),
            String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned(),
        )
    }

    fn combined_output(&self) -> String {
        let (stdout, stderr) = self.output();
        format!("stdout:\n{stdout}\nstderr:\n{stderr}")
    }

    fn has_exited(&mut self) -> bool {
        self.child.try_wait().unwrap().is_some()
    }

    fn wait_connected(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let output = self.combined_output();
            assert!(
                !self.has_exited(),
                "{} exited before WebRTC Connected:\n{output}",
                self.label
            );
            if peer_state_logged(&output, "Connected") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{} did not reach WebRTC Connected within {timeout:?}:\n{output}",
                self.label
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn finish(mut self, timeout: Duration) -> Completed {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let status = self.child.wait().unwrap();
                panic!(
                    "{} did not exit within {timeout:?}; status={status}:\n{}",
                    self.label,
                    self.combined_output()
                );
            }
            thread::sleep(Duration::from_millis(50));
        };
        self.join_drains();
        Completed {
            status,
            stdout: String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned(),
            stderr: String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned(),
        }
    }

    fn join_drains(&mut self) {
        for drain in self.drains.drain(..) {
            drain.join().expect("child output drain panicked");
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.join_drains();
    }
}

fn drain<R: Read + Send + 'static>(mut reader: R, sink: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => sink.lock().unwrap().extend_from_slice(&buffer[..read]),
            }
        }
    })
}

struct Completed {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Completed {
    fn diagnostics(&self) -> String {
        format!(
            "status={}\nstdout:\n{}\nstderr:\n{}",
            self.status, self.stdout, self.stderr
        )
    }
}

struct Stack {
    _server: ChildGuard,
    coturn: ChildGuard,
    topology: Topology,
}

impl Stack {
    fn start() -> Self {
        let topology = Topology::create();
        let config = topology.tempdir.path().join("live777.toml");
        fs::write(
            &config,
            format!(
                "[http]\nlisten = \"{SERVER}:{HTTP_PORT}\"\n\n[webrtc]\nice_udp_addrs = [\"{SERVER}:0\"]\nice_lite = true\n\n[log]\nlevel = \"info\"\n"
            ),
        )
        .unwrap();
        let mut server_command =
            topology.command(Namespace::Server, &bin_path!("live777"), Some(SERVER));
        server_command.args(["-v", "--config"]).arg(&config);
        let mut server = ChildGuard::spawn("live777", &mut server_command);
        wait_for_log(&mut server, "Server listening on", Duration::from_secs(10));

        let pidfile = topology.tempdir.path().join("coturn.pid");
        let mut turn_command = topology.command(Namespace::Turn, Path::new("turnserver"), None);
        turn_command.args([
            "-n",
            "--no-tls",
            "--no-dtls",
            "--no-cli",
            "--fingerprint",
            "--lt-cred-mech",
            "--realm=live777.local",
            "--user=live777:network-e2e",
            "--listening-ip=10.77.1.1",
            "--relay-ip=10.77.2.2",
            "--listening-port=3478",
            "--min-port=40000",
            "--max-port=40100",
            "--verbose",
        ]);
        turn_command.arg(format!("--pidfile={}", pidfile.display()));
        let coturn = ChildGuard::spawn("coturn", &mut turn_command);
        thread::sleep(Duration::from_millis(500));
        let mut stack = Self {
            _server: server,
            coturn,
            topology,
        };
        assert!(
            !stack.coturn.has_exited(),
            "coturn exited during startup:\n{}",
            stack.coturn.combined_output()
        );
        stack
    }

    fn publisher(&self, ice_server: &str, ice_addr: &str, stream: &str) -> ChildGuard {
        let mut command =
            self.topology
                .command(Namespace::Client, &bin_path!("whipsynth"), Some(ice_addr));
        command.args([
            "-v",
            "--whip",
            &format!("http://{SERVER}:{HTTP_PORT}/whip/{stream}"),
            "--vcodec=vp8",
            "--width=160",
            "--height=120",
            "--fps=15",
            "--ice-server",
            ice_server,
            "--timeout=30",
        ]);
        ChildGuard::spawn("whipsynth", &mut command)
    }

    fn probe_child(
        &self,
        ice_server: &str,
        ice_addr: &str,
        stream: &str,
        decode_secs: u64,
    ) -> ChildGuard {
        let mut command =
            self.topology
                .command(Namespace::Client, &bin_path!("whepprobe"), Some(ice_addr));
        command.args([
            "-v",
            "--whep",
            &format!("http://{SERVER}:{HTTP_PORT}/whep/{stream}"),
            "--output=json",
            "--decode-duration",
            &decode_secs.to_string(),
            "--timeout=20",
            "--ice-server",
            ice_server,
        ]);
        ChildGuard::spawn("whepprobe", &mut command)
    }

    fn probe(
        &self,
        ice_server: &str,
        ice_addr: &str,
        stream: &str,
        decode_secs: u64,
    ) -> ProbeOutput {
        let output = self
            .probe_child(ice_server, ice_addr, stream, decode_secs)
            .finish(Duration::from_secs(35));
        let report = serde_json::from_str(output.stdout.trim()).unwrap_or_else(|error| {
            panic!(
                "whepprobe stdout must contain only JSON: {error}\n{}",
                output.diagnostics()
            )
        });
        ProbeOutput { output, report }
    }
}

struct ProbeOutput {
    output: Completed,
    report: Value,
}

fn assert_media_success(probe: &ProbeOutput) {
    assert!(
        probe.output.status.success(),
        "probe failed:\n{}",
        probe.output.diagnostics()
    );
    assert_eq!(probe.report["backend"], "rsmpeg", "{}", probe.output.stdout);
    assert_eq!(probe.report["success"], true, "{}", probe.output.stdout);
    assert_eq!(probe.report["connected"], true, "{}", probe.output.stdout);
    assert_eq!(
        probe.report["video_codec"], "vp8",
        "{}",
        probe.output.stdout
    );
    assert_eq!(probe.report["width"], 160, "{}", probe.output.stdout);
    assert_eq!(probe.report["height"], 120, "{}", probe.output.stdout);
    assert_eq!(probe.report["video_tracks"], 1, "{}", probe.output.stdout);
    assert!(
        probe.report["frame_count"].as_u64().unwrap_or(0) > 0,
        "probe decoded no video frames:\n{}",
        probe.output.stdout
    );
    assert!(
        probe.report["video_bytes_received"].as_u64().unwrap_or(0) > 0,
        "probe received no video bytes:\n{}",
        probe.output.stdout
    );
}

fn peer_state_logged(output: &str, state: &str) -> bool {
    output.contains(&format!("Connection state changed: {state}"))
        || output.contains(&format!("WHEP connection state changed: {state}"))
}

fn assert_candidate(output: &str, kind: &str, address: &str) {
    let found = output.lines().any(|line| {
        let line = line.trim();
        if !line.starts_with("a=candidate:") {
            return false;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.len() >= 8
            && fields[4] == address
            && fields[6] == "typ"
            && fields[7] == kind
            && fields[4]
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| !ip.is_loopback())
    });
    assert!(
        found,
        "missing non-loopback {kind} candidate for {address}:\n{output}"
    );
}

fn wait_for_log(child: &mut ChildGuard, needle: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = child.combined_output();
        assert!(
            !child.has_exited(),
            "{} exited during startup:\n{output}",
            child.label
        );
        if output.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} did not log {needle:?} within {timeout:?}:\n{output}",
            child.label
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn preflight() {
    assert_eq!(
        std::env::var("LIVE777_NETWORK_E2E").as_deref(),
        Ok("1"),
        "network e2e is destructive to its private namespaces and requires LIVE777_NETWORK_E2E=1"
    );
    let status = fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let effective_uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1))
        .expect("effective UID in /proc/self/status");
    assert_eq!(
        effective_uid, "0",
        "network e2e requires root (effective UID 0) with CAP_NET_ADMIN"
    );
    let capabilities = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
        .expect("effective capabilities in /proc/self/status");
    assert_ne!(
        capabilities & (1 << 12),
        0,
        "network e2e requires CAP_NET_ADMIN for namespace, veth and tc setup"
    );
    for program in ["ip", "tc", "turnserver"] {
        find_program(program).unwrap_or_else(|| {
            panic!("network e2e requires {program} in PATH; install iproute2 and coturn")
        });
    }
    for (name, path) in [
        ("live777", bin_path!("live777")),
        ("whipsynth", bin_path!("whipsynth")),
        ("whepprobe", bin_path!("whepprobe")),
    ] {
        assert!(
            path.is_file(),
            "network e2e requires the real {name} binary at {}; run cargo test --features rsmpeg --test network_e2e --no-run first",
            path.display()
        );
    }
}

fn find_program(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let path = directory.join(program);
        let metadata = fs::metadata(&path).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(path)
    })
}

fn run_checked(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("could not execute {command:?}: {error}"));
    assert!(
        output.status.success(),
        "privileged network command failed (root with CAP_NET_ADMIN is required): {command:?}\n{}",
        Completed {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .diagnostics()
    );
    output
}

#[test]
#[ignore = "requires LIVE777_NETWORK_E2E=1, Linux root/CAP_NET_ADMIN, ip, tc, coturn and rsmpeg"]
fn non_loopback_stun_delivers_real_media() {
    let stack = Stack::start();
    let stun = format!("stun:{TURN_CLIENT}:{TURN_PORT}");
    let mut publisher = stack.publisher(&stun, CLIENT, "stun");
    publisher.wait_connected(Duration::from_secs(30));
    assert_candidate(&publisher.combined_output(), "srflx", CLIENT);
    let probe = stack.probe(&stun, CLIENT, "stun", 3);
    assert_media_success(&probe);
    assert_candidate(&probe.output.stderr, "srflx", CLIENT);
}

#[test]
#[ignore = "requires LIVE777_NETWORK_E2E=1, Linux root/CAP_NET_ADMIN, ip, tc, coturn and rsmpeg"]
fn forced_turn_relay_delivers_real_media() {
    let stack = Stack::start();
    let turn = format!("turn:{TURN_CLIENT}:{TURN_PORT}?transport=udp,live777,network-e2e");
    let mut publisher = stack.publisher(&turn, CLIENT_TURN, "relay");
    publisher.wait_connected(Duration::from_secs(30));
    assert_candidate(&publisher.combined_output(), "relay", TURN_SERVER);
    let probe = stack.probe(&turn, CLIENT_TURN, "relay", 3);
    assert_media_success(&probe);
    assert_candidate(&probe.output.stderr, "relay", TURN_SERVER);
}

#[test]
#[ignore = "requires LIVE777_NETWORK_E2E=1, Linux root/CAP_NET_ADMIN, ip, tc, coturn and rsmpeg"]
fn netem_loss_and_delay_deliver_real_media() {
    let stack = Stack::start();
    stack.topology.apply_netem();
    let mut publisher = stack.publisher("", CLIENT, "netem");
    publisher.wait_connected(Duration::from_secs(30));
    let probe = stack.probe("", CLIENT, "netem", 4);
    assert_media_success(&probe);
}

#[test]
#[ignore = "requires LIVE777_NETWORK_E2E=1, Linux root/CAP_NET_ADMIN, ip, tc, coturn and rsmpeg"]
fn two_second_outage_recovers_and_seven_second_outage_fails() {
    let stack = Stack::start();
    let mut publisher = stack.publisher("", CLIENT, "outage");
    publisher.wait_connected(Duration::from_secs(30));

    let mut recovery = stack.probe_child("", CLIENT, "outage", 10);
    recovery.wait_connected(Duration::from_secs(20));
    stack.topology.set_client_server_link(false);
    thread::sleep(Duration::from_secs(2));
    stack.topology.set_client_server_link(true);
    let recovery = recovery.finish(Duration::from_secs(30));
    let report: Value = serde_json::from_str(recovery.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "recovery probe stdout must contain only JSON: {error}\n{}",
            recovery.diagnostics()
        )
    });
    let recovered = ProbeOutput {
        output: recovery,
        report,
    };
    assert_media_success(&recovered);
    assert!(
        !publisher.has_exited(),
        "publisher died across a 2s outage:\n{}",
        publisher.combined_output()
    );

    let mut established = stack.probe_child("", CLIENT, "outage", 10);
    established.wait_connected(Duration::from_secs(20));
    stack.topology.set_client_server_link(false);
    let outage_started = Instant::now();
    thread::sleep(Duration::from_secs(7));

    let subscriber_output = established.combined_output();
    assert!(
        peer_state_logged(&subscriber_output, "Disconnected")
            || peer_state_logged(&subscriber_output, "Failed"),
        "established subscriber did not observe the 7s outage:\n{subscriber_output}"
    );
    let publisher_output = publisher.finish(Duration::from_secs(2));
    assert!(outage_started.elapsed() >= Duration::from_secs(7));
    assert!(
        !publisher_output.status.success(),
        "established publisher survived the 7s outage:\n{}",
        publisher_output.diagnostics()
    );
    let publisher_logs = format!("{}\n{}", publisher_output.stdout, publisher_output.stderr);
    assert!(
        peer_state_logged(&publisher_logs, "Disconnected")
            || peer_state_logged(&publisher_logs, "Failed"),
        "publisher exited without observing the network failure:\n{}",
        publisher_output.diagnostics()
    );
    drop(established);
    stack.topology.set_client_server_link(true);
}
