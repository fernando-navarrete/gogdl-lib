//! Offline test fixtures: a scripted local HTTP server, a temp dir and
//! zlib/MD5 helpers. Compiled for tests only.

use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use md5::{Digest, Md5};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

/// One scripted response.
#[derive(Clone)]
pub enum Reply {
    /// Drop the connection without answering (a transport error).
    Close,
    /// Answer with this status and an empty body.
    Status(u16),
    /// Answer `200` with this body.
    Body(Vec<u8>),
    /// Send the headers and the first `sent` bytes of `body`, signal
    /// `notify`, then hold the connection open until the client drops it.
    PartialThenHang {
        body: Vec<u8>,
        sent: usize,
        notify: Arc<Notify>,
    },
    /// Wait for `notify`, then send the inner reply.
    After(Arc<Notify>, Box<Reply>),
    /// Wait this long (real time), then send the inner reply.
    Delay(std::time::Duration, Box<Reply>),
}

struct Route {
    script: VecDeque<Reply>,
    last: Option<Reply>,
    requests: usize,
}

/// A local HTTP/1.1 server answering each path from a script. The last
/// reply of a script repeats once the script runs out. Every response is
/// `Connection: close`, so one request is one connection.
pub struct ChunkServer {
    addr: std::net::SocketAddr,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ChunkServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let accept_routes = routes.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, accept_routes.clone()));
            }
        });
        Self { addr, routes, task }
    }

    /// `http://127.0.0.1:<port>`, without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Sets the script for `path` (e.g. `/depot/ab/cd/abcd...`).
    pub fn script(&self, path: &str, replies: Vec<Reply>) {
        self.routes.lock().unwrap().insert(
            path.to_string(),
            Route {
                script: replies.into(),
                last: None,
                requests: 0,
            },
        );
    }

    /// How many requests `path` has received.
    pub fn requests(&self, path: &str) -> usize {
        self.routes
            .lock()
            .unwrap()
            .get(path)
            .map_or(0, |route| route.requests)
    }
}

impl Drop for ChunkServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut stream: TcpStream, routes: Arc<Mutex<HashMap<String, Route>>>) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();

    let reply = {
        let mut routes = routes.lock().unwrap();
        match routes.get_mut(&path) {
            None => Reply::Status(404),
            Some(route) => {
                route.requests += 1;
                if let Some(next) = route.script.pop_front() {
                    route.last = Some(next);
                }
                route.last.clone().unwrap_or(Reply::Status(404))
            }
        }
    };
    send(&mut stream, reply).await;
}

async fn send(stream: &mut TcpStream, reply: Reply) {
    let mut reply = reply;
    loop {
        match reply {
            Reply::Close => return,
            Reply::Status(code) => {
                let head =
                    format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                stream.write_all(head.as_bytes()).await.ok();
                return;
            }
            Reply::Body(body) => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.ok();
                stream.write_all(&body).await.ok();
                return;
            }
            Reply::PartialThenHang { body, sent, notify } => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.ok();
                stream.write_all(&body[..sent]).await.ok();
                stream.flush().await.ok();
                notify.notify_one();
                // Hold the connection until the client goes away.
                let mut sink = [0u8; 64];
                while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
                return;
            }
            Reply::Delay(wait, inner) => {
                tokio::time::sleep(wait).await;
                reply = *inner;
            }
            Reply::After(notify, inner) => {
                notify.notified().await;
                reply = *inner;
            }
        }
    }
}

/// A unique temp directory, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gogdl-lib-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// zlib-compresses `data`, as a chunk body on the wire.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Lowercase hex MD5 of `data`.
pub fn md5_hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

/// Runs `check` with `valid_until = now + offset` (Unix seconds) and returns
/// its result, redoing it if the wall clock ticked over a second in between,
/// so a boundary assertion can't flake.
pub fn with_valid_until(offset: i64, check: impl Fn(i64) -> bool) -> bool {
    loop {
        let now = chrono::Utc::now().timestamp();
        let result = check(now + offset);
        if chrono::Utc::now().timestamp() == now {
            return result;
        }
    }
}
