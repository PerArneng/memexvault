//! The activity stream's machinery: one reader of the log per vault, however many clients follow it, and a bounded buffer per client. A client waiting for frames holds no Postgres connection and no lease. The reader takes a pooled connection for one short statement at a time, and a client that needs older events than the ring holds takes turns for a page.
// This module waits on wall time between polls and checks how long a credential has gone unchecked, which is this edge's job.
#![allow(clippy::disallowed_methods)]
use crate::browser;
use crate::conn_watch::ConnTable;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, header};
use memex_api::{DurableFrame, EndFrame, HeartbeatFrame, HelloFrame, ProgressFrame, ResyncFrame, frame_of};
use memex_core::activity::{first_gap, plan_resume};
use memex_core::{ActivityLimits, Admission, FrameClass, ResumePlan, ResumeRequest, ResyncReason, Server, ServerEdges, ServerError, StoreError};
use memex_types::VaultId;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Semaphore};

/// How many clients read older events from the log at once, over all vaults, so a burst of reconnects can't take a connection each.
const CATCHUP_READERS: usize = 2;

/// One frame as bytes on the wire, shared by every client it goes to.
#[derive(Clone)]
struct Wire {
    class: FrameClass,
    seq: Option<i64>,
    bytes: Bytes,
}

fn sse(name: &str, id: Option<i64>, body: &impl Serialize) -> Bytes {
    // JSON has no raw line breaks, so one data line is always enough.
    let json = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let mut out = String::with_capacity(json.len() + 32);
    if let Some(id) = id {
        out.push_str("id: ");
        out.push_str(&id.to_string());
        out.push('\n');
    }
    out.push_str("event: ");
    out.push_str(name);
    out.push_str("\ndata: ");
    out.push_str(&json);
    out.push_str("\n\n");
    Bytes::from(out)
}

fn durable_wire(frame: &DurableFrame) -> Wire {
    Wire {
        class: FrameClass::Durable,
        seq: Some(frame.seq as i64),
        bytes: sse("durable", Some(frame.seq as i64), frame),
    }
}

fn resync_bytes(head: i64, reason: ResyncReason) -> Bytes {
    sse("resync", Some(head), &ResyncFrame::new(head, reason))
}

fn end_bytes(reason: &str) -> Bytes {
    sse("end", None, &EndFrame { reason: reason.into() })
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Bytes>,
    bytes: usize,
    ended: bool,
    /// Reading older events from the log, which an overflow makes pointless.
    catching_up: bool,
}

impl Queue {
    fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    fn push(&mut self, frame: Bytes) {
        self.bytes += frame.len();
        self.frames.push_back(frame);
    }
}

/// One open stream.
struct Client {
    queue: Mutex<Queue>,
    wake: Notify,
    /// Only the headers that say who this is, which get checked again while the stream lasts.
    credential: HeaderMap,
    verified_at: Mutex<Instant>,
}

impl Client {
    fn offer(&self, frame: &Wire, limits: ActivityLimits) {
        let mut q = self.queue.lock().expect("lock");
        if q.ended {
            return;
        }
        match memex_core::activity::admit(frame.class, q.frames.len(), q.bytes, frame.bytes.len(), limits.buffer) {
            Admission::Accept => q.push(frame.bytes.clone()),
            Admission::Drop => return,
            // Dropping what's waiting includes any older resync, so a client that never reads stays at one frame or a few.
            Admission::Overflow => {
                q.clear();
                q.catching_up = false;
                q.push(resync_bytes(frame.seq.unwrap_or(-1), ResyncReason::Overflow));
            }
        }
        drop(q);
        self.wake.notify_one();
    }

    /// Nothing after this: what was waiting goes too, as whoever this was may no longer see it.
    fn end(&self, reason: &str) {
        let mut q = self.queue.lock().expect("lock");
        if q.ended {
            return;
        }
        q.clear();
        q.catching_up = false;
        q.ended = true;
        q.push(end_bytes(reason));
        drop(q);
        self.wake.notify_one();
    }

    fn resync(&self, head: i64, reason: ResyncReason) {
        let mut q = self.queue.lock().expect("lock");
        if q.ended {
            return;
        }
        q.clear();
        q.catching_up = false;
        q.push(resync_bytes(head, reason));
        drop(q);
        self.wake.notify_one();
    }
}

struct FeedState {
    /// The last sequence published, where -1 is before the first event.
    position: i64,
    generation: u64,
    ring: VecDeque<Wire>,
    clients: Vec<Arc<Client>>,
}

/// One vault's single reader, and the clients it serves.
struct Feed {
    state: Mutex<FeedState>,
    limits: ActivityLimits,
}

impl Feed {
    fn publish(&self, frames: &[Wire]) {
        let mut state = self.state.lock().expect("lock");
        for frame in frames {
            if let Some(seq) = frame.seq {
                state.position = seq;
            }
            state.ring.push_back(frame.clone());
            if state.ring.len() > self.limits.ring_frames {
                state.ring.pop_front();
            }
            for client in &state.clients {
                client.offer(frame, self.limits);
            }
        }
    }

    fn offer_all(&self, frame: &Wire) {
        let state = self.state.lock().expect("lock");
        for client in &state.clients {
            client.offer(frame, self.limits);
        }
    }

    /// The log isn't what the reader thought it was, so everyone starts again from `head`.
    fn reset(&self, head: i64, generation: u64, reason: ResyncReason) {
        let mut state = self.state.lock().expect("lock");
        state.ring.clear();
        state.position = head;
        state.generation = generation;
        for client in &state.clients {
            client.resync(head, reason);
        }
    }

    fn end_all(&self, reason: &str) {
        let mut state = self.state.lock().expect("lock");
        for client in state.clients.drain(..) {
            client.end(reason);
        }
    }
}

/// A place in a principal's limit on streams. Giving it back is what closing a stream does.
pub(crate) struct StreamSlot {
    hub: Arc<Hub>,
    principal: String,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut streams = self.hub.streams.lock().expect("lock");
        if let Some(n) = streams.get_mut(&self.principal) {
            *n -= 1;
            if *n == 0 {
                streams.remove(&self.principal);
            }
        }
    }
}

/// All the streams of one server.
pub(crate) struct Hub {
    feeds: Mutex<HashMap<VaultId, Arc<Feed>>>,
    streams: Mutex<HashMap<String, usize>>,
    catchup: Semaphore,
    stopping: AtomicBool,
    conns: Arc<ConnTable>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            feeds: Mutex::default(),
            streams: Mutex::default(),
            catchup: Semaphore::new(CATCHUP_READERS),
            stopping: AtomicBool::new(false),
            conns: Arc::default(),
        }
    }
}

/// What a new stream starts with.
pub(crate) struct Opened {
    feed: Arc<Feed>,
    client: Arc<Client>,
    catchup: Option<Catchup>,
}

struct Catchup {
    after: i64,
    until: i64,
    page: VecDeque<Bytes>,
}

/// Why a stream wasn't opened.
pub(crate) struct TooManyStreams(pub(crate) usize);

fn credential_of(headers: &HeaderMap) -> HeaderMap {
    let mut kept = HeaderMap::new();
    for name in [header::AUTHORIZATION, header::COOKIE] {
        for value in headers.get_all(&name) {
            kept.append(name.clone(), value.clone());
        }
    }
    kept
}

/// Tells credentials apart without holding more than the headers already do.
fn credential_key(headers: &HeaderMap) -> Vec<u8> {
    let mut key = vec![];
    for name in [header::AUTHORIZATION, header::COOKIE] {
        for value in headers.get_all(&name) {
            key.extend_from_slice(HeaderValue::as_bytes(value));
            key.push(0);
        }
    }
    key
}

impl Hub {
    pub(crate) fn conns(&self) -> Arc<ConnTable> {
        self.conns.clone()
    }

    pub(crate) fn watch_connection(&self, peer: std::net::SocketAddr, stall_ms: u64) {
        self.conns.watch(peer, stall_ms);
    }

    /// Ends every stream, for shutdown. Their readers notice and stop.
    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        for feed in self.feeds.lock().expect("lock").values() {
            feed.end_all("server_stopping");
        }
    }

    pub(crate) fn take_slot(self: &Arc<Self>, principal: &str, limit: usize) -> Result<StreamSlot, TooManyStreams> {
        let mut streams = self.streams.lock().expect("lock");
        let n = streams.entry(principal.to_string()).or_insert(0);
        if *n >= limit {
            return Err(TooManyStreams(limit));
        }
        *n += 1;
        Ok(StreamSlot {
            hub: self.clone(),
            principal: principal.to_string(),
        })
    }

    /// Joins the vault's feed, starting its reader if this is the first client. `head` and `generation` are what the log said a moment ago, and only a new feed uses them. Everything from here on reaches this client's buffer, so what it reads from the log needs only go up to the feed's position, and nothing is missed or repeated.
    pub(crate) fn open<E>(
        self: &Arc<Self>,
        server: &Arc<Server<E>>,
        vault: &VaultId,
        head: i64,
        generation: u64,
        headers: &HeaderMap,
        request: ResumeRequest,
    ) -> Opened
    where
        E: ServerEdges + Send + Sync + 'static,
        Server<E>: Send + Sync,
    {
        let limits = server.activity_limits();
        let mut feeds = self.feeds.lock().expect("lock");
        let feed = feeds.get(vault).cloned().unwrap_or_else(|| {
            let feed = Arc::new(Feed {
                state: Mutex::new(FeedState {
                    position: head,
                    generation,
                    ring: VecDeque::new(),
                    clients: vec![],
                }),
                limits,
            });
            feeds.insert(vault.clone(), feed.clone());
            tokio::spawn(run_feed(self.clone(), server.clone(), vault.clone(), feed.clone()));
            feed
        });
        let client = Arc::new(Client {
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
            credential: credential_of(headers),
            verified_at: Mutex::new(Instant::now()),
        });
        let mut state = feed.state.lock().expect("lock");
        let position = state.position;
        let oldest = state.ring.front().and_then(|w| w.seq);
        let replay = limits.buffer.max_frames.saturating_sub(1);
        let mut catchup = None;
        {
            let mut q = client.queue.lock().expect("lock");
            let hello = |resumed: bool, id: Option<i64>| sse("hello", id, &HelloFrame { head: position, resumed });
            match plan_resume(request, position, oldest, replay) {
                // Its id means a client that hears nothing else can still resume from here.
                ResumePlan::Live { named: false } => q.push(hello(false, Some(position))),
                ResumePlan::Live { named: true } => q.push(hello(true, None)),
                ResumePlan::FromRing(after) => {
                    q.push(hello(true, None));
                    for frame in state.ring.iter().filter(|w| w.seq.is_some_and(|s| s > after)) {
                        q.push(frame.bytes.clone());
                    }
                }
                // The hello goes out with the catching up, ahead of anything live that queues up meanwhile.
                ResumePlan::FromLog { after, until } => {
                    q.catching_up = true;
                    catchup = Some(Catchup {
                        after,
                        until,
                        page: VecDeque::from([hello(true, None)]),
                    });
                }
                ResumePlan::Resync(reason) => {
                    q.push(hello(false, None));
                    q.push(resync_bytes(position, reason));
                }
            }
        }
        state.clients.push(client.clone());
        drop(state);
        drop(feeds);
        if self.stopping.load(Ordering::SeqCst) {
            client.end("server_stopping");
        }
        Opened { feed, client, catchup }
    }
}

/// The body of one stream. Closing it, by reaching the end or by the client going away, takes the client off its feed and gives back its slot.
pub(crate) fn body_of<E>(
    hub: Arc<Hub>,
    server: Arc<Server<E>>,
    vault: VaultId,
    opened: Opened,
    slot: StreamSlot,
    peer: Option<std::net::SocketAddr>,
) -> axum::body::Body
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    struct Leaving {
        feed: Arc<Feed>,
        client: Arc<Client>,
        /// The connection outlives its stream, and whatever it serves next isn't watched.
        watched: Option<(Arc<ConnTable>, std::net::SocketAddr)>,
        _slot: StreamSlot,
    }
    impl Drop for Leaving {
        fn drop(&mut self) {
            if let Some((conns, peer)) = &self.watched {
                conns.watch(*peer, 0);
            }
            self.feed.state.lock().expect("lock").clients.retain(|c| !Arc::ptr_eq(c, &self.client));
        }
    }
    struct Stream<E: ServerEdges> {
        hub: Arc<Hub>,
        server: Arc<Server<E>>,
        vault: VaultId,
        client: Arc<Client>,
        catchup: Option<Catchup>,
        _leaving: Leaving,
    }
    let watched = peer.map(|p| (hub.conns(), p));
    let start = Stream {
        hub,
        server,
        vault,
        client: opened.client.clone(),
        catchup: opened.catchup,
        _leaving: Leaving {
            feed: opened.feed,
            client: opened.client,
            watched,
            _slot: slot,
        },
    };
    let frames = futures_util::stream::unfold(start, |mut s| async move {
        loop {
            enum Next {
                Frame(Bytes),
                Finished,
                Wait,
                CatchUp,
            }
            let next = {
                let mut q = s.client.queue.lock().expect("lock");
                // What's in the queue comes after what it's catching up on, unless the stream has ended or overflowed, which stops the catching up.
                if q.catching_up {
                    Next::CatchUp
                } else if let Some(frame) = q.frames.pop_front() {
                    q.bytes -= frame.len();
                    Next::Frame(frame)
                } else if q.ended {
                    Next::Finished
                } else {
                    Next::Wait
                }
            };
            let catchup = match next {
                Next::Frame(frame) => return Some((Ok::<Bytes, Infallible>(frame), s)),
                Next::Finished => return None,
                Next::Wait => {
                    s.client.wake.notified().await;
                    continue;
                }
                Next::CatchUp => match s.catchup.as_mut() {
                    Some(c) => c,
                    None => {
                        s.client.queue.lock().expect("lock").catching_up = false;
                        continue;
                    }
                },
            };
            if let Some(frame) = catchup.page.pop_front() {
                return Some((Ok(frame), s));
            }
            if catchup.after >= catchup.until {
                s.client.queue.lock().expect("lock").catching_up = false;
                continue;
            }
            let Ok(_turn) = s.hub.catchup.acquire().await else { return None };
            let (server, vault, after, until) = (s.server.clone(), s.vault.clone(), catchup.after, catchup.until);
            let read = tokio::task::spawn_blocking(move || read_page(&server, &vault, after)).await;
            match read {
                Ok(Ok(page)) => {
                    let gap = first_gap(after, page.iter().map(|w| w.seq.unwrap_or(-1) as u64));
                    let last = page.last().and_then(|w| w.seq);
                    let kept = page.into_iter().take(gap.unwrap_or(usize::MAX)).filter(|w| w.seq.is_some_and(|q| q <= until));
                    catchup.page.extend(kept.map(|w| w.bytes));
                    match (gap, last) {
                        // Past the feed's position, or nothing at all, means this client has everything up to it.
                        (None, Some(l)) => catchup.after = l.min(until),
                        (None, None) | (Some(_), _) => {
                            catchup.page.push_back(resync_bytes(until, ResyncReason::LogGap));
                            catchup.after = until;
                        }
                    }
                }
                _ => {
                    // It can carry on from its last id once the store is back.
                    s.client.end("unavailable");
                }
            }
        }
    });
    axum::body::Body::from_stream(frames)
}

/// A page of the log as frames, on a blocking thread.
fn read_page<E: ServerEdges>(server: &Server<E>, vault: &VaultId, after: i64) -> Result<Vec<Wire>, ServerError> {
    let events = server.activity_events(vault, u64::try_from(after).ok())?;
    Ok(events.iter().map(|e| durable_wire(&frame_of(e))).collect())
}

/// How one credential's check came out.
enum Verdict {
    Allowed,
    End(&'static str),
    Unknown,
}

fn verdict_of(result: Result<(), ServerError>) -> Verdict {
    match result {
        Ok(()) => Verdict::Allowed,
        Err(ServerError::Unauthenticated) => Verdict::End("auth_ended"),
        Err(ServerError::Forbidden | ServerError::NotFound) => Verdict::End("scope_lost"),
        Err(_) => Verdict::Unknown,
    }
}

async fn recheck<E>(server: &Arc<Server<E>>, vault: &VaultId, feed: &Feed)
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let clients: Vec<Arc<Client>> = feed.state.lock().expect("lock").clients.clone();
    let mut by_credential: HashMap<Vec<u8>, Vec<Arc<Client>>> = HashMap::new();
    for client in clients {
        by_credential.entry(credential_key(&client.credential)).or_default().push(client);
    }
    let groups: Vec<Vec<Arc<Client>>> = by_credential.into_values().collect();
    let credentials: Vec<HeaderMap> = groups.iter().filter_map(|g| g.first().map(|c| c.credential.clone())).collect();
    let (s, v) = (server.clone(), vault.clone());
    // One after another on one thread, so however many there are this holds one connection at a time.
    let checked = tokio::task::spawn_blocking(move || -> Vec<Result<(), ServerError>> {
        credentials
            .iter()
            .map(|headers| browser::caller_from(&s, headers).and_then(|caller| s.activity_allowed(&caller, &v)))
            .collect()
    })
    .await;
    let Ok(checked) = checked else { return };
    let mut ended = vec![];
    for (group, result) in groups.iter().zip(checked) {
        match verdict_of(result) {
            Verdict::Allowed => {
                for client in group {
                    *client.verified_at.lock().expect("lock") = Instant::now();
                }
            }
            Verdict::End(reason) => {
                for client in group {
                    client.end(reason);
                    ended.push(client.clone());
                }
            }
            // Failing closed at once would end every stream at the first hiccup, so it gets a while, during which no new check has passed.
            Verdict::Unknown => {
                for client in group {
                    if client.verified_at.lock().expect("lock").elapsed() >= Duration::from_millis(feed.limits.unverified_ms) {
                        client.end("unverifiable");
                        ended.push(client.clone());
                    }
                }
            }
        }
    }
    if !ended.is_empty() {
        feed.state.lock().expect("lock").clients.retain(|c| !ended.iter().any(|e| Arc::ptr_eq(c, e)));
    }
}

/// Reads what's new in the log, one page, and says whether there's more waiting.
async fn advance<E>(server: &Arc<Server<E>>, vault: &VaultId, feed: &Feed) -> Result<bool, StoreError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (s, v) = (server.clone(), vault.clone());
    let marks = tokio::task::spawn_blocking(move || s.activity_marks(&v))
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))??;
    let head = marks.head.map_or(-1, |h| h as i64);
    let (position, generation) = {
        let state = feed.state.lock().expect("lock");
        (state.position, state.generation)
    };
    if marks.generation != generation {
        feed.reset(head, marks.generation, ResyncReason::Erased);
        return Ok(false);
    }
    if head < position {
        feed.reset(head, marks.generation, ResyncReason::LogGap);
        return Ok(false);
    }
    if head == position {
        return Ok(false);
    }
    let (s, v) = (server.clone(), vault.clone());
    let page = tokio::task::spawn_blocking(move || read_page(&s, &v, position))
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?
        .map_err(|e| StoreError::Backend(e.to_string()))?;
    match first_gap(position, page.iter().map(|w| w.seq.unwrap_or(-1) as u64)) {
        None if page.is_empty() => {
            feed.reset(head, marks.generation, ResyncReason::LogGap);
            Ok(false)
        }
        None => {
            feed.publish(&page);
            Ok(page.last().and_then(|w| w.seq).is_some_and(|l| l < head))
        }
        Some(at) => {
            feed.publish(&page[..at]);
            feed.reset(head, marks.generation, ResyncReason::LogGap);
            Ok(false)
        }
    }
}

/// One vault's reader, for as long as anyone follows the vault.
async fn run_feed<E>(hub: Arc<Hub>, server: Arc<Server<E>>, vault: VaultId, feed: Arc<Feed>)
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let limits = feed.limits;
    let mut progress_version = server.activity_progress(&vault, u64::MAX).1;
    let (mut next_recheck, mut next_heartbeat) = (Instant::now(), Instant::now() + Duration::from_millis(limits.heartbeat_ms));
    let mut more = false;
    loop {
        if !more {
            tokio::time::sleep(Duration::from_millis(limits.poll_ms)).await;
        }
        if hub.stopping.load(Ordering::SeqCst) {
            feed.end_all("server_stopping");
            hub.feeds.lock().expect("lock").remove(&vault);
            return;
        }
        {
            // Checked under the hub's lock, so a client joining now can't join a feed that's going away.
            let mut feeds = hub.feeds.lock().expect("lock");
            if feed.state.lock().expect("lock").clients.is_empty() {
                feeds.remove(&vault);
                return;
            }
        }
        more = match advance(&server, &vault, &feed).await {
            Ok(more) => more,
            Err(StoreError::UnknownVault { .. }) => {
                feed.end_all("vault_gone");
                hub.feeds.lock().expect("lock").remove(&vault);
                return;
            }
            Err(e) => {
                tracing::warn!(vault = %vault, error = %e, "the activity stream can't read the log");
                false
            }
        };
        let (reports, version) = server.activity_progress(&vault, progress_version);
        progress_version = version;
        for report in &reports {
            if let Some(frame) = ProgressFrame::of(report) {
                feed.offer_all(&Wire {
                    class: FrameClass::Progress,
                    seq: None,
                    bytes: sse("progress", None, &frame),
                });
            }
        }
        let now = Instant::now();
        if now >= next_heartbeat {
            next_heartbeat = now + Duration::from_millis(limits.heartbeat_ms);
            let head = feed.state.lock().expect("lock").position;
            feed.offer_all(&Wire {
                class: FrameClass::Heartbeat,
                seq: None,
                bytes: sse("heartbeat", None, &HeartbeatFrame { head }),
            });
        }
        if now >= next_recheck {
            next_recheck = now + Duration::from_millis(limits.recheck_ms);
            recheck(&server, &vault, &feed).await;
        }
    }
}
