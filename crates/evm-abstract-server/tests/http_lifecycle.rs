//! Process-level checks distinguish idle closure from a broken HTTP request.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Server {
    child: Child,
    address: SocketAddr,
    assets: PathBuf,
    logs: Arc<(Mutex<String>, Condvar)>,
    reader: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let assets = std::env::temp_dir().join(format!(
            "evm-http-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&assets).unwrap();
        fs::write(assets.join("index.html"), "complete response").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_evm-abstract-server"))
            .args(["--bind", "127.0.0.1:0", "--workers", "1", "--assets"])
            .arg(&assets)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .strip_prefix("EVM workbench: http://")
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let logs = Arc::new((Mutex::new(String::new()), Condvar::new()));
        let destination = Arc::clone(&logs);
        let stderr = child.stderr.take().unwrap();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let mut logs = destination.0.lock().unwrap();
                logs.push_str(&line.unwrap());
                logs.push('\n');
                destination.1.notify_all();
            }
        });
        Self {
            child,
            address,
            assets,
            logs,
            reader: Some(reader),
        }
    }

    fn connect(&self) -> TcpStream {
        let socket = TcpStream::connect(self.address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        socket
    }

    fn finish(mut self) -> String {
        // EOF can precede the server's final error write. A fresh parse failure
        // fences the single-threaded host, and its complete stderr line fences
        // the reader, without guessing how long either process needs to run.
        let mut probe = self.connect();
        let marker = format!("HTTP connection from {}:", probe.local_addr().unwrap());
        probe.write_all(b"invalid\r\n\r\n").unwrap();
        probe.read_to_end(&mut Vec::new()).unwrap();
        let (logs, ready) = &*self.logs;
        let (logs, _) = ready
            .wait_timeout_while(logs.lock().unwrap(), Duration::from_secs(5), |logs| {
                !logs.lines().any(|line| line.starts_with(&marker))
            })
            .unwrap();
        let observed = logs.lines().any(|line| line.starts_with(&marker));
        let failure_logs = logs.clone();
        drop(logs);
        assert!(observed, "{failure_logs}");
        let _ = self.child.kill();
        self.child.wait().unwrap();
        // Drain all emitted stderr before checking absence or presence of errors.
        self.reader.take().unwrap().join().unwrap();
        self.logs
            .0
            .lock()
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with(&marker))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        fs::remove_dir_all(&self.assets).unwrap();
    }
}

fn complete_response(socket: &mut TcpStream) {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("HTTP/1.1 200"), "{line}");
    let mut length = None;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.unwrap()];
    reader.read_exact(&mut body).unwrap();
    assert_eq!(body, b"complete response");
}

fn expired(socket: &mut TcpStream, started: Instant) {
    let mut after = Vec::new();
    socket.read_to_end(&mut after).unwrap();
    assert!(started.elapsed() >= Duration::from_secs(9));
    assert!(
        after.is_empty() || after.starts_with(b"HTTP/1.1 408"),
        "{after:?}"
    );
}

#[test]
fn unused_connection_expires_without_an_error_log() {
    let server = Server::new();
    let mut socket = server.connect();
    expired(&mut socket, Instant::now());
    assert!(server.finish().is_empty());
}

#[test]
fn completed_response_then_idle_expires_quietly_and_host_stays_responsive() {
    let server = Server::new();
    let mut socket = server.connect();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    complete_response(&mut socket);
    expired(&mut socket, Instant::now());
    let mut fresh = server.connect();
    fresh
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    complete_response(&mut fresh);
    assert_eq!(fresh.read(&mut [0]).unwrap(), 0);
    assert!(server.finish().is_empty());
}

#[test]
fn unfinished_initial_header_still_expires_with_peer_and_error_log() {
    let server = Server::new();
    let mut socket = server.connect();
    let peer = socket.local_addr().unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    expired(&mut socket, Instant::now());
    let logs = server.finish();
    assert!(logs.contains("read header from client timeout"), "{logs}");
    assert!(logs.contains(&peer.to_string()), "{logs}");
}

#[test]
fn pipelined_partial_header_is_not_mistaken_for_idle() {
    let server = Server::new();
    let mut socket = server.connect();
    // Hyper can read both requests in one socket read. The second head stays
    // inside Hyper's buffer, so service-boundary byte counters cannot detect it.
    socket
        .write_all(
            b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nGET / HTTP/1.1\r\nHost: localhost\r\n",
        )
        .unwrap();
    complete_response(&mut socket);
    expired(&mut socket, Instant::now());
    let logs = server.finish();
    assert!(logs.contains("read header from client timeout"), "{logs}");
}

#[test]
fn keepalive_reuse_then_idle_still_expires_quietly() {
    let server = Server::new();
    let mut socket = server.connect();
    for _ in 0..2 {
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        complete_response(&mut socket);
    }
    expired(&mut socket, Instant::now());
    assert!(server.finish().is_empty());
}
