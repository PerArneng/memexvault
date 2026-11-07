//! A small blocking client for server-sent events, on a plain socket, so a test can stop reading or go away whenever it likes and see exactly the bytes sent.
#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: Option<String>,
    pub event: String,
    pub data: String,
}

impl Frame {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.data).unwrap_or_else(|e| panic!("{e}: {}", self.data))
    }
}

pub struct Sse {
    pub socket: TcpStream,
    pub status: u16,
    pub headers: String,
    /// Bytes read and not yet de-chunked.
    wire: Vec<u8>,
    /// The body so far, not yet made into frames.
    body: Vec<u8>,
    /// Every byte of the body, to scan afterwards.
    pub recorded: Vec<u8>,
    pub closed: bool,
}

pub fn request_text(path: &str, headers: &[(&str, &str)]) -> String {
    let mut text = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n");
    for (k, v) in headers {
        text.push_str(&format!("{k}: {v}\r\n"));
    }
    text.push_str("\r\n");
    text
}

/// Sends the request and reads as far as the end of the response's headers.
pub fn open(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Sse {
    let mut socket = TcpStream::connect(addr).expect("connect");
    socket.set_nodelay(true).ok();
    socket.write_all(request_text(path, headers).as_bytes()).expect("send");
    socket.set_read_timeout(Some(Duration::from_millis(50))).ok();
    let mut sse = Sse {
        socket,
        status: 0,
        headers: String::new(),
        wire: vec![],
        body: vec![],
        recorded: vec![],
        closed: false,
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(end) = sse.wire.windows(4).position(|w| w == b"\r\n\r\n") {
            sse.headers = String::from_utf8_lossy(&sse.wire[..end]).into_owned();
            sse.status = sse.headers.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            sse.wire.drain(..end + 4);
            return sse;
        }
        assert!(Instant::now() < deadline, "no response from {path}");
        assert!(!sse.closed, "closed before answering {path}");
        sse.fill();
    }
}

impl Sse {
    fn fill(&mut self) {
        let mut chunk = [0u8; 65536];
        match self.socket.read(&mut chunk) {
            Ok(0) => self.closed = true,
            Ok(n) => self.wire.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => self.closed = true,
        }
    }

    fn dechunk(&mut self) {
        while let Some(line_end) = self.wire.windows(2).position(|w| w == b"\r\n") {
            let size = usize::from_str_radix(String::from_utf8_lossy(&self.wire[..line_end]).trim(), 16).expect("a chunk size");
            if size == 0 {
                self.closed = true;
                self.wire.clear();
                return;
            }
            if self.wire.len() < line_end + 2 + size + 2 {
                return;
            }
            let chunk: Vec<u8> = self.wire[line_end + 2..line_end + 2 + size].to_vec();
            self.recorded.extend_from_slice(&chunk);
            self.body.extend_from_slice(&chunk);
            self.wire.drain(..line_end + 2 + size + 2);
        }
    }

    fn frame(&mut self) -> Option<Frame> {
        let end = self.body.windows(2).position(|w| w == b"\n\n")?;
        let text = String::from_utf8_lossy(&self.body[..end]).into_owned();
        self.body.drain(..end + 2);
        let mut frame = Frame {
            id: None,
            event: "message".into(),
            data: String::new(),
        };
        for line in text.lines() {
            match line.split_once(": ") {
                Some(("id", v)) => frame.id = Some(v.into()),
                Some(("event", v)) => frame.event = v.into(),
                Some(("data", v)) => frame.data.push_str(v),
                _ => {}
            }
        }
        Some(frame)
    }

    /// The next frame, or none if the time runs out or the stream ends first.
    pub fn next(&mut self, within: Duration) -> Option<Frame> {
        let deadline = Instant::now() + within;
        loop {
            self.dechunk();
            if let Some(frame) = self.frame() {
                return Some(frame);
            }
            if self.closed || Instant::now() >= deadline {
                return None;
            }
            self.fill();
        }
    }

    /// Frames until one of this name, which is last in the answer.
    pub fn until(&mut self, event: &str, within: Duration) -> Vec<Frame> {
        let deadline = Instant::now() + within;
        let mut out = vec![];
        while let Some(frame) = self.next(deadline.saturating_duration_since(Instant::now())) {
            let done = frame.event == event;
            out.push(frame);
            if done {
                return out;
            }
        }
        panic!("no {event} frame, after {out:?}");
    }

    /// The rest of a plain response's body, for an error.
    pub fn plain_body(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let length: Option<usize> = self
            .headers
            .lines()
            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_string))
            .and_then(|v| v.trim().parse().ok());
        while Instant::now() < deadline && !self.closed && length.is_some_and(|n| self.wire.len() < n) {
            self.fill();
        }
        String::from_utf8_lossy(&self.wire).into_owned()
    }
}

pub fn seq_of(frame: &Frame) -> i64 {
    frame.id.as_deref().expect("an id").parse().expect("a number")
}

/// A plain keep-alive HTTP/1.1 connection, for requests that should cost nothing but themselves.
pub struct Http {
    socket: TcpStream,
    addr: SocketAddr,
    token: String,
}

impl Http {
    pub fn new(addr: SocketAddr, token: &str) -> Self {
        let socket = TcpStream::connect(addr).expect("connect");
        socket.set_nodelay(true).ok();
        socket.set_read_timeout(Some(Duration::from_secs(60))).ok();
        Self {
            socket,
            addr,
            token: token.into(),
        }
    }

    /// Sends a request and reads its answer, which must have a length or be chunked.
    pub fn call(&mut self, method: &str, path: &str, content_type: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            self.token,
            body.len()
        );
        self.socket.write_all(head.as_bytes()).expect("send");
        self.socket.write_all(body).expect("send");
        let mut got = vec![];
        let mut chunk = [0u8; 65536];
        let end = loop {
            if let Some(end) = got.windows(4).position(|w: &[u8]| w == b"\r\n\r\n") {
                break end;
            }
            let n = self.socket.read(&mut chunk).expect("read");
            assert!(n > 0, "closed in the headers from {:?}", self.addr);
            got.extend_from_slice(&chunk[..n]);
        };
        let headers = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
        let status = headers.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut rest = got[end + 4..].to_vec();
        if let Some(length) = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length: "))
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            while rest.len() < length {
                let n = self.socket.read(&mut chunk).expect("read");
                assert!(n > 0, "closed in the body");
                rest.extend_from_slice(&chunk[..n]);
            }
            return (status, rest);
        }
        // Chunked.
        let mut body = vec![];
        loop {
            let Some(line_end) = rest.windows(2).position(|w| w == b"\r\n") else {
                let n = self.socket.read(&mut chunk).expect("read");
                assert!(n > 0, "closed in a chunk");
                rest.extend_from_slice(&chunk[..n]);
                continue;
            };
            let size = usize::from_str_radix(String::from_utf8_lossy(&rest[..line_end]).trim(), 16).expect("a chunk size");
            while rest.len() < line_end + 2 + size + 2 {
                let n = self.socket.read(&mut chunk).expect("read");
                assert!(n > 0, "closed in a chunk");
                rest.extend_from_slice(&chunk[..n]);
            }
            body.extend_from_slice(&rest[line_end + 2..line_end + 2 + size]);
            rest.drain(..line_end + 2 + size + 2);
            if size == 0 {
                return (status, body);
            }
        }
    }

    pub fn json(&mut self, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
        let text = body.map_or(String::new(), |b| b.to_string());
        let (status, bytes) = self.call(method, path, "application/json", text.as_bytes());
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }
}
