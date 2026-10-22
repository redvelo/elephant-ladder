//! Bounded, session-local CDP reducer. No complete URL, headers or body survives parsing.
use crate::Rejected;
use serde::Deserialize;
use std::collections::HashMap;

pub const MAX_MESSAGE: usize = 64 * 1024; // Extracted string metadata, not raw payload.
pub const MAX_FRAMES: usize = 256;
pub const MAX_REQUESTS: usize = 1024;
pub const MAX_ID: usize = 512;
const MAX_URL: usize = 8192;

struct Text<const N: usize>(String);
impl<'de, const N: usize> Deserialize<'de> for Text<N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor<const N: usize>;
        impl<'de, const N: usize> serde::de::Visitor<'de> for Visitor<N> {
            type Value = Text<N>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "at most {N} bytes of metadata")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > N {
                    return Err(E::custom("metadata limit"));
                }
                Ok(Text(value.to_owned()))
            }
        }
        d.deserialize_str(Visitor::<N>)
    }
}
fn id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Text::<MAX_ID>::deserialize(d)?.0)
}
fn optional_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<Text<MAX_ID>>::deserialize(d)?.map(|s| s.0))
}

#[derive(Debug)]
pub struct Message {
    pub session: Option<String>,
    pub event: Event,
}

pub trait Sink: Clone {
    fn session_id(&self) -> &str;
    fn active(&self) -> bool;
    fn observe(&self, at: u64);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    id: Option<i32>,
    #[serde(default, deserialize_with = "optional_id")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "optional_id")]
    method: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Wire {
    id: Option<i32>,
    #[serde(default, deserialize_with = "optional_id")]
    method: Option<String>,
    error: Option<serde::de::IgnoredAny>,
    result: Option<Reply>,
    #[serde(default)]
    params: Params,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Reply {
    frame_tree: Option<Tree>,
}

struct Tree {
    frames: Vec<SnapshotFrame>,
}

// Flatten while deserializing so an owned reply cannot allocate an arbitrarily
// large recursive tree before the frame/metadata limits are checked.
impl<'de> Deserialize<'de> for Tree {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::DeserializeSeed;
        let (mut frames, mut count, mut bytes) = (Vec::new(), 0, 0);
        TreeSeed {
            frames: &mut frames,
            count: &mut count,
            bytes: &mut bytes,
            depth: 0,
        }
        .deserialize(d)?;
        Ok(Self { frames })
    }
}
struct TreeSeed<'a> {
    frames: &'a mut Vec<SnapshotFrame>,
    count: &'a mut usize,
    bytes: &'a mut usize,
    depth: usize,
}
#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum TreeField {
    Frame,
    ChildFrames,
    #[serde(other)]
    Other,
}
impl<'de> serde::de::DeserializeSeed<'de> for TreeSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.depth >= 64 {
            return Err(serde::de::Error::custom("tree depth limit"));
        }
        d.deserialize_map(self)
    }
}
impl<'de> serde::de::Visitor<'de> for TreeSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded frame tree")
    }
    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let start = self.frames.len();
        let (mut frame, mut children) = (None, false);
        while let Some(key) = map.next_key::<TreeField>()? {
            match key {
                TreeField::Frame => {
                    if frame.is_some() || *self.count >= MAX_FRAMES {
                        return Err(serde::de::Error::custom("frame limit"));
                    }
                    *self.count += 1;
                    let value: SnapshotFrame = map.next_value()?;
                    *self.bytes += value.metadata_len();
                    if *self.bytes > MAX_MESSAGE {
                        return Err(serde::de::Error::custom("metadata limit"));
                    }
                    frame = Some(value);
                }
                TreeField::ChildFrames => {
                    if children {
                        return Err(serde::de::Error::custom("duplicate children"));
                    }
                    children = true;
                    map.next_value_seed(ChildrenSeed(TreeSeed {
                        frames: self.frames,
                        count: self.count,
                        bytes: self.bytes,
                        depth: self.depth + 1,
                    }))?;
                }
                TreeField::Other => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        self.frames.insert(
            start,
            frame.ok_or_else(|| serde::de::Error::custom("missing frame"))?,
        );
        Ok(())
    }
}
struct ChildrenSeed<'a>(TreeSeed<'a>);
impl<'de> serde::de::DeserializeSeed<'de> for ChildrenSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for ChildrenSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded child frames")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq
            .next_element_seed(TreeSeed {
                frames: self.0.frames,
                count: self.0.count,
                bytes: self.0.bytes,
                depth: self.0.depth,
            })?
            .is_some()
        {}
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFrame {
    #[serde(deserialize_with = "id")]
    id: String,
    #[serde(default, deserialize_with = "optional_id")]
    parent_id: Option<String>,
    #[serde(deserialize_with = "id")]
    loader_id: String,
    #[serde(default, rename = "url", deserialize_with = "location")]
    location: Location,
}

impl SnapshotFrame {
    pub fn id(&self) -> &str {
        &self.id
    }
    fn metadata_len(&self) -> usize {
        self.id.len()
            + self.parent_id.as_ref().map_or(0, String::len)
            + self.loader_id.len()
            + self.location.origin.len()
    }
}

#[derive(Debug, Default)]
struct Location {
    origin: String,
    inheritable: bool,
}

fn location<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Location, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Location;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a frame URL")
        }
        fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Location, E> {
            // Inspect the borrowed string before parsing/copying it: data URLs
            // can contain entire images. Keep a revoking navigation, not an error.
            if text.len() > MAX_URL {
                return Ok(Location::default());
            }
            let Ok(url) = url::Url::parse(text) else {
                return Ok(Location::default());
            };
            let inheritable = remote(&url) || matches!(text, "about:blank" | "about:srcdoc");
            if !matches!(url.scheme(), "http" | "https") && !inheritable {
                return Ok(Location::default());
            }
            let origin = url.origin().ascii_serialization();
            if origin.len() > MAX_URL {
                return Ok(Location::default());
            }
            Ok(Location {
                origin,
                inheritable,
            })
        }
    }
    deserializer.deserialize_str(Visitor)
}

fn remote_url<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a network URL")
        }
        fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<bool, E> {
            Ok(text.len() <= MAX_URL && url::Url::parse(text).is_ok_and(|url| remote(&url)))
        }
    }
    deserializer.deserialize_str(Visitor)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Params {
    #[serde(default, deserialize_with = "optional_id")]
    reason: Option<String>,
    #[serde(default, deserialize_with = "optional_id")]
    session_id: Option<String>,
    target_info: Option<TargetInfo>,
    frame: Option<Document>,
    #[serde(default, deserialize_with = "optional_id")]
    frame_id: Option<String>,
    #[serde(default, deserialize_with = "optional_id")]
    parent_frame_id: Option<String>,
    #[serde(default, deserialize_with = "optional_id")]
    request_id: Option<String>,
    #[serde(default, deserialize_with = "optional_id")]
    loader_id: Option<String>,
    request: Option<RequestUrl>,
    response: Option<Response>,
    redirect_response: Option<serde::de::IgnoredAny>,
    encoded_data_length: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo {
    #[serde(deserialize_with = "id")]
    target_id: String,
    #[serde(rename = "type", deserialize_with = "id")]
    kind: String,
    #[serde(default, deserialize_with = "optional_id")]
    parent_frame_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(deserialize_with = "id")]
    id: String,
    #[serde(default, deserialize_with = "optional_id")]
    parent_id: Option<String>,
    #[serde(deserialize_with = "id")]
    loader_id: String,
    #[serde(rename = "url", deserialize_with = "location")]
    location: Location,
}

#[derive(Deserialize)]
struct RequestUrl {
    #[serde(rename = "url", deserialize_with = "remote_url")]
    remote: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    #[serde(rename = "url", deserialize_with = "remote_url")]
    remote: bool,
    #[serde(default, rename = "remoteIPAddress", deserialize_with = "optional_id")]
    remote_ip_address: Option<String>,
    from_disk_cache: Option<bool>,
    from_service_worker: Option<bool>,
    from_prefetch_cache: Option<bool>,
    status: f64,
}

#[derive(Debug)]
pub enum Event {
    TargetAttached {
        session: String,
        target: String,
        parent_frame: Option<String>,
    },
    TargetDetached(String),
    Ack(i32, bool, Vec<SnapshotFrame>),
    Snapshot(Vec<SnapshotFrame>),
    Disconnected,
    Navigation {
        frame: String,
        parent: Option<String>,
        loader: String,
        origin: String,
        inheritable: bool,
    },
    Attached(String, String),
    Detached(String),
    FrameSwapped(String),
    Request {
        id: String,
        frame: String,
        loader: String,
        remote: bool,
        redirect: bool,
    },
    Response(String, bool),
    Cached(String),
    Data(String, bool),
    End(String),
    Ignore,
}

fn remote(url: &url::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Err(_) => {
            let host = host.trim_end_matches('.');
            host != "localhost" && !host.ends_with(".localhost")
        }
        Ok(ip) => remote_ip(ip),
    }
}

fn remote_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_unspecified(),
        std::net::IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && ip.to_ipv4_mapped().is_none_or(|ip| remote_ip(ip.into()))
        }
    }
}

#[cfg(test)]
pub fn parse(raw: &[u8]) -> Result<Message, Rejected> {
    parse_owned(raw, |_, _| true)
}

pub fn parse_owned(
    raw: &[u8],
    owns_reply: impl FnOnce(&str, i32) -> bool,
) -> Result<Message, Rejected> {
    // serde skips unknown envelope values without retaining them. Raw byte size
    // says nothing about relevance: WebSocket bodies and foreign replies can be huge.
    let envelope: Envelope = serde_json::from_slice(raw).map_err(|_| Rejected)?;
    if envelope
        .id
        .is_some_and(|id| !owns_reply(envelope.session_id.as_deref().unwrap_or(""), id))
    {
        return Ok(Message {
            session: None,
            event: Event::Ignore,
        });
    }
    if envelope
        .session_id
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > MAX_ID)
    {
        return Err(Rejected);
    }
    let event = parse_event(raw, &envelope)?;
    let strings = match &event {
        Event::Ack(_, _, frames) | Event::Snapshot(frames) => {
            frames.iter().map(SnapshotFrame::metadata_len).sum()
        }
        Event::Navigation {
            frame,
            parent,
            loader,
            origin,
            ..
        } => frame.len() + parent.as_ref().map_or(0, String::len) + loader.len() + origin.len(),
        Event::TargetAttached {
            session,
            target,
            parent_frame,
        } => session.len() + target.len() + parent_frame.as_ref().map_or(0, String::len),
        Event::Request {
            id, frame, loader, ..
        } => id.len() + frame.len() + loader.len(),
        Event::Attached(frame, parent) => frame.len() + parent.len(),
        Event::TargetDetached(id)
        | Event::Detached(id)
        | Event::FrameSwapped(id)
        | Event::Response(id, _)
        | Event::Cached(id)
        | Event::Data(id, _)
        | Event::End(id) => id.len(),
        Event::Disconnected | Event::Ignore => 0,
    };
    if strings + envelope.session_id.as_ref().map_or(0, String::len) > MAX_MESSAGE {
        return Err(Rejected);
    }
    Ok(Message {
        session: envelope.session_id,
        event,
    })
}

fn parse_event(raw: &[u8], envelope: &Envelope) -> Result<Event, Rejected> {
    // Other domains (notably WebSocket handshake/frame events) reuse field names
    // with incompatible schemas. Do not deserialize their parameters as HTTP.
    if envelope.id.is_none()
        && !matches!(
            envelope.method.as_deref(),
            Some(
                "Inspector.detached"
                    | "Target.attachedToTarget"
                    | "Target.detachedFromTarget"
                    | "Inspector.targetCrashed"
                    | "Page.frameNavigated"
                    | "Page.frameAttached"
                    | "Page.frameDetached"
                    | "Network.requestWillBeSent"
                    | "Network.responseReceived"
                    | "Network.requestServedFromCache"
                    | "Network.dataReceived"
                    | "Network.loadingFinished"
                    | "Network.loadingFailed"
            )
        )
    {
        return Ok(Event::Ignore);
    }
    let wire: Wire = serde_json::from_slice(raw).map_err(|_| Rejected)?;
    if let Some(id) = wire.id {
        let success = wire.error.is_none() && wire.result.is_some();
        let frames = wire
            .result
            .and_then(|r| r.frame_tree)
            .map_or_else(Vec::new, |tree| tree.frames);
        return Ok(Event::Ack(id, success, frames));
    }
    let p = wire.params;
    let required = |s: Option<String>| {
        s.filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or(Rejected)
    };
    Ok(match wire.method.as_deref() {
        Some("Target.attachedToTarget") => {
            let target = p.target_info.ok_or(Rejected)?;
            if target.kind != "iframe" {
                return Ok(Event::Ignore);
            }
            Event::TargetAttached {
                session: required(p.session_id)?,
                target: required(Some(target.target_id))?,
                parent_frame: target
                    .parent_frame_id
                    .map(|s| required(Some(s)))
                    .transpose()?,
            }
        }
        Some("Target.detachedFromTarget") => Event::TargetDetached(required(p.session_id)?),
        Some("Inspector.detached" | "Inspector.targetCrashed") => Event::Disconnected,
        Some("Page.frameNavigated") => {
            let f = p.frame.ok_or(Rejected)?;
            Event::Navigation {
                frame: required(Some(f.id))?,
                parent: f.parent_id.map(|s| required(Some(s))).transpose()?,
                loader: required(Some(f.loader_id))?,
                origin: f.location.origin,
                inheritable: f.location.inheritable,
            }
        }
        Some("Page.frameAttached") => {
            Event::Attached(required(p.frame_id)?, required(p.parent_frame_id)?)
        }
        Some("Page.frameDetached") if p.reason.as_deref() == Some("swap") => {
            Event::FrameSwapped(required(p.frame_id)?)
        }
        Some("Page.frameDetached") => Event::Detached(required(p.frame_id)?),
        Some("Network.requestWillBeSent")
            if p.frame_id.as_ref().is_some_and(|s| !s.is_empty())
                && p.loader_id.as_ref().is_some_and(|s| !s.is_empty()) =>
        {
            Event::Request {
                id: required(p.request_id)?,
                frame: required(p.frame_id)?,
                loader: required(p.loader_id)?,
                remote: p.request.is_some_and(|r| r.remote),
                redirect: p.redirect_response.is_some(),
            }
        }
        Some("Network.responseReceived") => {
            let r = p.response.ok_or(Rejected)?;
            Event::Response(
                required(p.request_id)?,
                // CDP declares these flags optional, without specifying an absent
                // value as false. Keep explicit negatives for these two sources.
                r.from_disk_cache == Some(false)
                    && r.from_service_worker == Some(false)
                    && r.from_prefetch_cache != Some(true)
                    && r.status != 304.0
                    && (100.0..600.0).contains(&r.status)
                    && r.remote_ip_address
                        // Chromium reports IPv6 remote endpoints in brackets.
                        .and_then(|s| {
                            s.strip_prefix('[')
                                .and_then(|s| s.strip_suffix(']'))
                                .unwrap_or(&s)
                                .parse()
                                .ok()
                        })
                        .is_some_and(remote_ip)
                    && r.remote,
            )
        }
        Some("Network.requestServedFromCache") => Event::Cached(required(p.request_id)?),
        Some("Network.dataReceived") => Event::Data(
            required(p.request_id)?,
            p.encoded_data_length
                .is_some_and(|n| n.is_finite() && n > 0.0),
        ),
        Some("Network.loadingFinished" | "Network.loadingFailed") => {
            Event::End(required(p.request_id)?)
        }
        _ => Event::Ignore,
    })
}

struct Frame<S> {
    parent: Option<String>,
    loader: String,
    sink: Option<S>,
}

struct Request<S> {
    sink: S,
    response: bool,
}

pub struct Reducer<S> {
    frames: HashMap<String, Frame<S>>,
    requests: HashMap<String, Request<S>>,
    pub failed: bool,
    external_parent: Option<(String, S)>,
}

impl<S: Sink> Default for Reducer<S> {
    fn default() -> Self {
        Self {
            frames: HashMap::new(),
            requests: HashMap::new(),
            failed: false,
            external_parent: None,
        }
    }
}

impl<S: Sink> Reducer<S> {
    pub fn fail(&mut self) {
        self.frames.clear();
        self.requests.clear();
        self.external_parent = None;
        self.failed = true;
    }

    pub fn owner(&self, frame: &str) -> Option<S> {
        self.frames.get(frame)?.sink.clone().filter(Sink::active)
    }

    pub fn set_external_parent(&mut self, parent: Option<(String, S)>) {
        self.external_parent = parent;
    }

    // Only the target coordinator can authorize this one-shot exception for a
    // newly attached, browser-related iframe. Root snapshots use Event::Snapshot.
    pub fn initial_child_snapshot(
        &mut self,
        frames: Vec<SnapshotFrame>,
        session: Option<&str>,
        bind: impl Fn(&str) -> Option<S>,
    ) -> bool {
        let mut bound = false;
        for (index, f) in frames.into_iter().enumerate() {
            if self
                .frames
                .get(&f.id)
                .is_some_and(|frame| !frame.loader.is_empty())
            {
                continue;
            }
            if f.loader_id.is_empty() {
                continue;
            }
            let parent = f.parent_id.or_else(|| {
                (index == 0)
                    .then(|| self.external_parent.as_ref().map(|p| p.0.clone()))
                    .flatten()
            });
            let frame = f.id.clone();
            self.apply(
                Event::Navigation {
                    frame: f.id,
                    parent,
                    loader: f.loader_id,
                    origin: f.location.origin,
                    inheritable: f.location.inheritable,
                },
                session,
                0,
                &bind,
            );
            bound |= self.owner(&frame).is_some();
        }
        bound
    }

    fn detach(&mut self, id: &str) {
        let mut pending = vec![id.to_owned()];
        while let Some(id) = pending.pop() {
            pending.extend(
                self.frames
                    .iter()
                    .filter(|(_, f)| f.parent.as_deref() == Some(&id))
                    .map(|(id, _)| id.clone()),
            );
            self.frames.remove(&id);
        }
    }

    pub fn apply(
        &mut self,
        event: Event,
        session: Option<&str>,
        at: u64,
        bind: impl Fn(&str) -> Option<S>,
    ) {
        if self.failed {
            return;
        }
        match event {
            Event::Disconnected => self.fail(),
            Event::Snapshot(frames) => {
                for frame in frames {
                    // A tree is only a baseline. It must never authorize an existing
                    // document, even when its origin matches the current publication.
                    self.frames.entry(frame.id).or_insert(Frame {
                        parent: frame.parent_id,
                        loader: frame.loader_id,
                        sink: None,
                    });
                }
            }
            Event::Navigation {
                frame,
                parent,
                loader,
                origin,
                inheritable,
            } => {
                // Duplicate commits must not rebind an old document after same-origin reopen.
                if !origin.is_empty() && self.frames.get(&frame).is_some_and(|f| f.loader == loader)
                {
                    return;
                }
                self.detach(&frame);
                let sink = (if origin.is_empty() {
                    None
                } else {
                    bind(&origin)
                })
                .or_else(|| {
                    if !inheritable {
                        return None;
                    }
                    let parent = parent.as_ref()?;
                    self.frames
                        .get(parent)
                        .and_then(|f| f.sink.clone())
                        .or_else(|| {
                            self.external_parent
                                .as_ref()
                                .filter(|(id, _)| id == parent)
                                .map(|(_, sink)| sink.clone())
                        })
                })
                .filter(|s| s.active() && Some(s.session_id()) == session);
                self.frames.insert(
                    frame,
                    Frame {
                        parent,
                        loader,
                        sink,
                    },
                );
            }
            Event::Attached(frame, parent) => {
                self.frames.entry(frame).or_insert(Frame {
                    parent: Some(parent),
                    loader: String::new(),
                    sink: None,
                });
            }
            Event::Detached(frame) | Event::FrameSwapped(frame) => self.detach(&frame),
            Event::Request {
                id,
                frame,
                loader,
                remote,
                redirect,
            } => {
                // Redirect chains are deliberately discarded, never rebound mid-request.
                if self.requests.remove(&id).is_some() || redirect || !remote {
                    return;
                }
                if let Some(sink) = self
                    .frames
                    .get(&frame)
                    .filter(|f| f.loader == loader)
                    .and_then(|f| f.sink.clone())
                    .filter(|s| s.active() && Some(s.session_id()) == session)
                {
                    self.requests.insert(
                        id,
                        Request {
                            sink,
                            response: false,
                        },
                    );
                }
            }
            Event::Response(id, credible) => {
                if credible {
                    if let Some(r) = self.requests.get_mut(&id) {
                        r.response = true;
                        r.sink.observe(at);
                    }
                } else {
                    self.requests.remove(&id);
                }
            }
            Event::Data(id, true) => {
                if let Some(r) = self.requests.get(&id).filter(|r| r.response) {
                    r.sink.observe(at);
                }
            }
            Event::Cached(id) | Event::End(id) => {
                self.requests.remove(&id);
            }
            _ => {}
        }
        if self.frames.len() > MAX_FRAMES || self.requests.len() > MAX_REQUESTS {
            self.fail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(raw: &[u8]) -> Result<Event, Rejected> {
        super::parse(raw).map(|message| message.event)
    }
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone)]
    struct TestSink {
        id: &'static str,
        active: Arc<AtomicBool>,
        seen: Arc<Mutex<Vec<u64>>>,
    }
    impl TestSink {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                active: Arc::new(AtomicBool::new(true)),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }
    impl Sink for TestSink {
        fn session_id(&self) -> &str {
            self.id
        }
        fn active(&self) -> bool {
            self.active.load(Ordering::Acquire)
        }
        fn observe(&self, at: u64) {
            if self.active() {
                self.seen.lock().unwrap().push(at);
            }
        }
    }
    fn nav(
        frame: &str,
        parent: Option<&str>,
        loader: &str,
        origin: &str,
        inheritable: bool,
    ) -> Event {
        Event::Navigation {
            frame: frame.into(),
            parent: parent.map(str::to_owned),
            loader: loader.into(),
            origin: origin.into(),
            inheritable,
        }
    }
    fn request(id: &str, frame: &str, loader: &str) -> Event {
        Event::Request {
            id: id.into(),
            frame: frame.into(),
            loader: loader.into(),
            remote: true,
            redirect: false,
        }
    }
    fn apply(r: &mut Reducer<TestSink>, event: Event, sink: &TestSink, at: u64) {
        r.apply(event, Some(sink.id), at, |origin| {
            (origin == "http://127.0.0.1:4000").then(|| sink.clone())
        });
    }
    fn owned(sink: &TestSink) -> Reducer<TestSink> {
        let mut r = Reducer::default();
        apply(
            &mut r,
            nav("root", None, "doc", "http://127.0.0.1:4000", false),
            sink,
            0,
        );
        r
    }

    #[test]
    fn network_protocol_request_response_and_positive_wire_data_only() {
        let s = TestSink::new("one");
        let mut r = owned(&s);
        apply(&mut r, request("r", "root", "doc"), &s, 1);
        apply(&mut r, Event::Data("r".into(), true), &s, 2);
        assert!(s.seen.lock().unwrap().is_empty());
        apply(&mut r, Event::Response("r".into(), true), &s, 3);
        apply(&mut r, Event::Data("r".into(), false), &s, 4);
        apply(&mut r, Event::Data("r".into(), true), &s, 5);
        apply(&mut r, Event::End("r".into()), &s, 6);
        apply(&mut r, Event::Data("r".into(), true), &s, 7);
        assert_eq!(*s.seen.lock().unwrap(), [3, 5]);
    }

    #[test]
    fn network_protocol_same_origin_reopen_never_rebinds_existing_frames_or_requests() {
        let old = TestSink::new("old");
        let new = TestSink::new("new");
        let mut r = owned(&old);
        apply(&mut r, request("old-request", "root", "doc"), &old, 1);
        old.active.store(false, Ordering::Release);
        apply(
            &mut r,
            nav("root", None, "doc", "http://127.0.0.1:4000", false),
            &new,
            2,
        );
        apply(&mut r, request("new-request", "root", "doc"), &new, 3);
        apply(&mut r, Event::Response("new-request".into(), true), &new, 4);
        apply(
            &mut r,
            nav("root", None, "fresh", "http://127.0.0.1:4000", false),
            &new,
            5,
        );
        apply(&mut r, Event::Response("old-request".into(), true), &new, 6);
        assert!(old.seen.lock().unwrap().is_empty());
        assert!(new.seen.lock().unwrap().is_empty());
        apply(&mut r, request("fresh-request", "root", "fresh"), &new, 7);
        apply(
            &mut r,
            Event::Response("fresh-request".into(), true),
            &new,
            8,
        );
        assert_eq!(*new.seen.lock().unwrap(), [8]);
    }

    #[test]
    fn network_protocol_navigation_receipt_session_must_match_binding() {
        let s = TestSink::new("new");
        let mut r = Reducer::default();
        r.apply(
            nav("root", None, "doc", "http://127.0.0.1:4000", false),
            Some("old"),
            0,
            |_| Some(s.clone()),
        );
        apply(&mut r, request("r", "root", "doc"), &s, 1);
        apply(&mut r, Event::Response("r".into(), true), &s, 2);
        assert!(s.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn network_protocol_descendants_need_proven_parent_and_measurement_cannot_inherit() {
        let s = TestSink::new("one");
        let mut r = owned(&s);
        apply(
            &mut r,
            nav(
                "child",
                Some("root"),
                "child-doc",
                "https://remote.example",
                true,
            ),
            &s,
            0,
        );
        apply(
            &mut r,
            nav(
                "measurement",
                Some("root"),
                "measure-doc",
                "http://127.0.0.1:4001",
                false,
            ),
            &s,
            0,
        );
        apply(
            &mut r,
            nav(
                "orphan",
                Some("missing"),
                "orphan-doc",
                "https://remote.example",
                true,
            ),
            &s,
            0,
        );
        for (frame, doc) in [
            ("child", "child-doc"),
            ("measurement", "measure-doc"),
            ("orphan", "orphan-doc"),
        ] {
            apply(&mut r, request(frame, frame, doc), &s, 1);
            apply(&mut r, Event::Response(frame.into(), true), &s, 2);
        }
        assert_eq!(*s.seen.lock().unwrap(), [2]);
        apply(&mut r, Event::Detached("root".into()), &s, 3);
        assert!(!r.frames.contains_key("child"));
        assert!(!r.frames.contains_key("measurement"));
    }

    #[test]
    fn network_protocol_snapshot_cannot_authorize_or_overwrite_fresh_navigation() {
        let s = TestSink::new("one");
        let mut r = Reducer::default();
        let snapshot = || {
            Event::Snapshot(vec![SnapshotFrame {
                id: "root".into(),
                parent_id: None,
                loader_id: "doc".into(),
                location: Location::default(),
            }])
        };
        apply(&mut r, snapshot(), &s, 0);
        apply(
            &mut r,
            nav("root", None, "doc", "http://127.0.0.1:4000", false),
            &s,
            0,
        );
        assert!(r.frames["root"].sink.is_none());
        apply(
            &mut r,
            nav("root", None, "fresh", "http://127.0.0.1:4000", false),
            &s,
            0,
        );
        apply(&mut r, snapshot(), &s, 0);
        assert_eq!(r.frames["root"].loader, "fresh");
        assert!(r.frames["root"].sink.is_some());
    }

    #[test]
    fn network_protocol_cache_redirect_local_and_wrong_loader_are_ignored() {
        let s = TestSink::new("one");
        let mut r = owned(&s);
        for event in [
            request("cache", "root", "doc"),
            Event::Cached("cache".into()),
            request("worker", "root", "doc"),
            Event::Response("worker".into(), false),
            request("redirect", "root", "doc"),
            Event::Request {
                id: "redirect".into(),
                frame: "root".into(),
                loader: "doc".into(),
                remote: true,
                redirect: true,
            },
            Event::Request {
                id: "local".into(),
                frame: "root".into(),
                loader: "doc".into(),
                remote: false,
                redirect: false,
            },
            request("wrong", "root", "other-doc"),
        ] {
            apply(&mut r, event, &s, 1);
        }
        for id in ["cache", "worker", "redirect", "local", "wrong"] {
            apply(&mut r, Event::Response(id.into(), true), &s, 2);
        }
        assert!(s.seen.lock().unwrap().is_empty());
        assert!(r.requests.is_empty());
    }

    #[test]
    fn network_protocol_frame_and_request_overflow_clear_and_latch() {
        let s = TestSink::new("one");
        let mut r = owned(&s);
        for n in 0..MAX_REQUESTS {
            apply(&mut r, request(&n.to_string(), "root", "doc"), &s, 0);
        }
        assert!(!r.failed);
        apply(&mut r, request("overflow", "root", "doc"), &s, 0);
        assert!(r.failed && r.frames.is_empty() && r.requests.is_empty());
        apply(
            &mut r,
            nav("root", None, "fresh", "http://127.0.0.1:4000", false),
            &s,
            0,
        );
        assert!(r.frames.is_empty());
        let mut r = Reducer::default();
        for n in 0..=MAX_FRAMES {
            apply(&mut r, Event::Attached(n.to_string(), "root".into()), &s, 0);
        }
        assert!(r.failed && r.frames.is_empty());
    }

    #[test]
    fn network_protocol_raw_child_sessions_and_unrelated_events_do_not_collide() {
        assert!(matches!(
            parse(br#"{"sessionId":"child","id":7,"result":{}}"#).unwrap(),
            Event::Ack(7, true, _)
        ));
        assert!(matches!(parse(br#"{"sessionId":"child","method":"Page.frameDetached","params":{"frameId":"root"}}"#).unwrap(), Event::Detached(_)));
        assert!(matches!(
            parse(br#"{"id":7,"result":{}}"#).unwrap(),
            Event::Ack(7, true, _)
        ));
        assert!(matches!(
            parse(br#"{"id":7,"error":{"code":-1}}"#).unwrap(),
            Event::Ack(7, false, _)
        ));
        assert!(matches!(
            parse(
                br#"{"method":"Network.webSocketCreated","params":{"url":"wss://remote.example"}}"#
            )
            .unwrap(),
            Event::Ignore
        ));
        assert!(matches!(parse(br#"{"method":"Network.requestWillBeSent","params":{"requestId":"worker","loaderId":""}}"#).unwrap(), Event::Ignore));
    }

    #[test]
    fn network_protocol_parser_filters_cache_service_worker_and_nonwire_data() {
        for (flags, credible) in [
            (
                r#""fromDiskCache":false,"fromServiceWorker":false,"status":200"#,
                true,
            ),
            (
                r#""fromDiskCache":true,"fromServiceWorker":false,"status":200"#,
                false,
            ),
            (
                r#""fromDiskCache":false,"fromServiceWorker":true,"status":200"#,
                false,
            ),
            (
                r#""fromDiskCache":false,"fromServiceWorker":false,"fromPrefetchCache":true,"status":200"#,
                false,
            ),
            (
                r#""fromDiskCache":false,"fromServiceWorker":false,"status":304"#,
                false,
            ),
            (r#""status":200"#, false),
        ] {
            let raw = format!(
                r#"{{"method":"Network.responseReceived","params":{{"requestId":"r","response":{{"url":"https://remote.example/path?secret","remoteIPAddress":"8.8.8.8",{flags}}}}}}}"#
            );
            assert!(
                matches!(parse(raw.as_bytes()).unwrap(), Event::Response(_, value) if value == credible)
            );
        }
        for (length, expected) in [(0, false), (-1, false), (1, true)] {
            let raw = format!(
                r#"{{"method":"Network.dataReceived","params":{{"requestId":"r","dataLength":99,"encodedDataLength":{length}}}}}"#
            );
            assert!(
                matches!(parse(raw.as_bytes()).unwrap(), Event::Data(_, value) if value == expected)
            );
        }
    }

    #[test]
    fn network_protocol_parser_discards_url_paths_headers_and_payloads() {
        let event = parse(br#"{"method":"Page.frameNavigated","params":{"frame":{"id":"root","loaderId":"doc","url":"http://127.0.0.1:4000/secret?token=private"}}}"#).unwrap();
        assert!(
            matches!(&event, Event::Navigation { origin, .. } if origin == "http://127.0.0.1:4000")
        );
        assert!(!format!("{event:?}").contains("secret"));
        let event = parse(br#"{"method":"Network.requestWillBeSent","params":{"frameId":"root","loaderId":"doc","requestId":"r","request":{"url":"https://remote.example/secret","headers":{"Authorization":"private"},"postData":"payload"}}}"#).unwrap();
        let debug = format!("{event:?}");
        for secret in [
            "remote.example",
            "secret",
            "private",
            "payload",
            "Authorization",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(parse(&vec![b' '; MAX_MESSAGE + 1]).is_err());
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn network_protocol_https_allows_lan_and_excludes_loopback_app_hosts() {
        for url in [
            "http://remote.example",
            "wss://remote.example",
            "https://localhost",
            "https://x.localhost",
            "https://localhost.",
            "https://reader.elephant-ladder.localhost",
            "https://p-publication.localhost",
            "https://n-publication.localhost",
            "https://m-publication.localhost",
            "https://127.0.0.1",
            "https://127.123.45.67",
            "https://0.0.0.0",
            "https://[::1]",
            "https://[::]",
            "https://[::ffff:127.0.0.1]",
        ] {
            assert!(!remote(&url::Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "https://remote.example",
            "https://8.8.8.8",
            "https://[2606:4700:4700::1111]",
            "https://printer.local",
            "https://intranet",
            "https://server.internal",
            "https://10.0.0.1",
            "https://169.254.1.1",
            "https://192.168.1.1",
            "https://100.64.0.1",
            "https://192.0.2.1",
            "https://[fd00::1]",
            "https://[fe80::1]",
            "https://[2001:db8::1]",
            "https://[::ffff:192.168.1.1]",
        ] {
            assert!(remote(&url::Url::parse(url).unwrap()), "{url}");
        }
    }

    #[test]
    fn network_protocol_websocket_parameters_are_not_parsed_as_http() {
        for raw in [
            r#"{"method":"Network.webSocketFrameReceived","params":{"response":{"opcode":1,"mask":false,"payloadData":"private"}}}"#,
            r#"{"method":"Network.webSocketWillSendHandshakeRequest","params":{"request":{"headers":{"Authorization":"private"}}}}"#,
        ] {
            assert!(matches!(parse(raw.as_bytes()).unwrap(), Event::Ignore));
        }
    }

    #[test]
    fn network_protocol_response_rejects_loopback_dns_destinations_and_missing_ip() {
        assert!(matches!(parse(br#"{"method":"Network.responseReceived","params":{"requestId":"r","response":{"url":"https://remote.example","status":200,"fromDiskCache":false,"fromServiceWorker":false}}}"#).unwrap(), Event::Response(_, false)));
        for ip in [
            "",
            "invalid",
            "127.0.0.1",
            "127.10.20.30",
            "::1",
            "::ffff:127.0.0.1",
            "0.0.0.0",
            "::",
        ] {
            let raw = format!(
                r#"{{"method":"Network.responseReceived","params":{{"requestId":"r","response":{{"url":"https://remote.example","remoteIPAddress":"{ip}","status":200,"fromDiskCache":false,"fromServiceWorker":false}}}}}}"#
            );
            assert!(matches!(
                parse(raw.as_bytes()).unwrap(),
                Event::Response(_, false)
            ));
        }
    }

    #[test]
    fn network_protocol_lan_https_response_records_real_traffic() {
        let s = TestSink::new("one");
        let mut reducer = owned(&s);
        for (n, ip) in [
            "10.0.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
            "192.0.2.1",
        ]
        .into_iter()
        .enumerate()
        {
            let request = format!(
                r#"{{"method":"Network.requestWillBeSent","params":{{"requestId":"{n}","frameId":"root","loaderId":"doc","request":{{"url":"https://printer.local/job"}}}}}}"#
            );
            apply(&mut reducer, parse(request.as_bytes()).unwrap(), &s, 1);
            let response = format!(
                r#"{{"method":"Network.responseReceived","params":{{"requestId":"{n}","response":{{"url":"https://printer.local/job","remoteIPAddress":"{ip}","status":200,"fromDiskCache":false,"fromServiceWorker":false}}}}}}"#
            );
            apply(&mut reducer, parse(response.as_bytes()).unwrap(), &s, 2);
        }
        assert_eq!(*s.seen.lock().unwrap(), [2; 6]);
    }

    #[test]
    fn network_protocol_frame_tree_parsing_and_disconnect_are_conservative() {
        let event = parse(br#"{"id":9,"result":{"frameTree":{"frame":{"id":"root","loaderId":"doc","url":"http://127.0.0.1:4000/private"},"childFrames":[{"frame":{"id":"child","parentId":"root","loaderId":"child-doc"}}]}}}"#).unwrap();
        let Event::Ack(9, true, frames) = event else {
            panic!("missing tree")
        };
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].parent_id.as_deref(), Some("root"));
        let s = TestSink::new("one");
        let mut r = owned(&s);
        apply(
            &mut r,
            parse(br#"{"method":"Inspector.detached","params":{"reason":"replaced"}}"#).unwrap(),
            &s,
            0,
        );
        assert!(r.failed && r.frames.is_empty());
    }

    #[test]
    fn network_protocol_chromium_bracketed_ipv6_metadata_is_supported() {
        for (ip, eligible) in [
            ("[2001:db8::1]", true),
            ("[fd00::1]", true),
            ("[::1]", false),
            ("[::ffff:127.0.0.1]", false),
            ("[[fd00::1]]", false),
        ] {
            let raw = serde_json::to_vec(
                &serde_json::json!({"method":"Network.responseReceived","params":{
                "requestId":"r","response":{"url":"https://lan.internal/","remoteIPAddress":ip,
                    "status":200,"fromDiskCache":false,"fromServiceWorker":false}}}),
            )
            .unwrap();
            assert!(matches!(parse(&raw).unwrap(), Event::Response(_, value) if value == eligible));
        }
    }

    #[test]
    fn network_protocol_owned_tree_enforces_frame_and_total_metadata_budgets() {
        let frame = |id: String, loader: String| {
            serde_json::json!({"frame":{
            "id":id,"loaderId":loader,"url":"http://p-book.localhost/"}})
        };
        for children in [MAX_FRAMES - 1, MAX_FRAMES] {
            let raw = serde_json::to_vec(&serde_json::json!({"id":1,"result":{"frameTree":{
                "frame":{"id":"root","loaderId":"doc"},
                "childFrames":(0..children).map(|i| frame(i.to_string(), "doc".into())).collect::<Vec<_>>()}}})).unwrap();
            assert_eq!(parse(&raw).is_ok(), children < MAX_FRAMES);
        }
        let raw = serde_json::to_vec(&serde_json::json!({"id":1,"result":{"frameTree":{
            "frame":{"id":"root","loaderId":"doc"},
            "childFrames":(0..100).map(|i| frame(format!("{i}{}", "x".repeat(500)), "y".repeat(500))).collect::<Vec<_>>()}}})).unwrap();
        assert!(parse(&raw).is_err());
        assert!(matches!(
            super::parse_owned(&raw, |_, _| false).unwrap().event,
            Event::Ignore
        ));
    }

    #[test]
    fn network_protocol_large_ignored_response_data_does_not_expand_metadata() {
        let raw = serde_json::to_vec(
            &serde_json::json!({"method":"Network.dataReceived","params":{
            "requestId":"r","encodedDataLength":100,"data":"x".repeat(MAX_MESSAGE * 4)}}),
        )
        .unwrap();
        assert!(matches!(parse(&raw).unwrap(), Event::Data(_, true)));
        let raw = serde_json::to_vec(&serde_json::json!({"method":"Page.frameNavigated","params":{
            "frame":{"id":"f","loaderId":"l","url":format!("http://localhost/{}", "x".repeat(MAX_URL))}}})).unwrap();
        assert!(
            matches!(parse(&raw).unwrap(), Event::Navigation { origin, inheritable: false, .. } if origin.is_empty())
        );
    }

    #[test]
    fn network_protocol_oversized_data_urls_are_nonremote_without_failing() {
        let url = format!("data:image/png;base64,{}", "A".repeat(MAX_MESSAGE * 2));
        let s = TestSink::new("reader");
        let mut r = owned(&s);
        let raw = serde_json::to_vec(
            &serde_json::json!({"method":"Network.requestWillBeSent","params":{
            "frameId":"root","loaderId":"doc","requestId":"data","request":{"url":url}}}),
        )
        .unwrap();
        let event = parse(&raw).unwrap();
        assert!(matches!(&event, Event::Request { remote: false, .. }));
        apply(&mut r, event, &s, 1);
        assert!(r.requests.is_empty());

        apply(&mut r, request("response", "root", "doc"), &s, 2);
        let raw = serde_json::to_vec(
            &serde_json::json!({"method":"Network.responseReceived","params":{
            "requestId":"response","response":{"url":url,"status":200,"fromDiskCache":false,
                "fromServiceWorker":false,"remoteIPAddress":"192.168.1.1"}}}),
        )
        .unwrap();
        let event = parse(&raw).unwrap();
        assert!(matches!(&event, Event::Response(_, false)));
        apply(&mut r, event, &s, 3);
        apply(&mut r, Event::Data("response".into(), true), &s, 4);
        assert!(r.requests.is_empty());
        assert!(s.seen.lock().unwrap().is_empty());
        assert!(!r.failed);

        let raw = serde_json::to_vec(&serde_json::json!({"id":1,"result":{"frameTree":{
            "frame":{"id":"snapshot","loaderId":"doc","url":url}}}}))
        .unwrap();
        let Event::Ack(_, true, frames) = parse(&raw).unwrap() else {
            panic!("missing snapshot")
        };
        assert!(frames[0].location.origin.is_empty());
        assert!(!frames[0].location.inheritable);
    }

    #[test]
    fn network_protocol_rejected_frame_urls_revoke_old_owner_and_descendants() {
        for url in [
            format!("data:image/png;base64,{}", "A".repeat(MAX_MESSAGE * 2)),
            "blob:http://127.0.0.1:4000/old".into(),
            "not a URL".into(),
        ] {
            for loader in ["doc", "next-doc"] {
                let s = TestSink::new("reader");
                let mut r = owned(&s);
                apply(
                    &mut r,
                    nav(
                        "child",
                        Some("root"),
                        "child-doc",
                        "https://remote.example",
                        true,
                    ),
                    &s,
                    0,
                );
                assert!(r.owner("child").is_some());
                let raw = serde_json::to_vec(
                    &serde_json::json!({"method":"Page.frameNavigated","params":{
                    "frame":{"id":"root","loaderId":loader,"url":url}}}),
                )
                .unwrap();
                let event = parse(&raw).unwrap();
                assert!(
                    matches!(&event, Event::Navigation { origin, inheritable: false, .. } if origin.is_empty())
                );
                apply(&mut r, event, &s, 1);
                assert!(r.owner("root").is_none());
                assert!(!r.frames.contains_key("child"));
                apply(&mut r, request("later", "root", loader), &s, 2);
                apply(&mut r, Event::Response("later".into(), true), &s, 3);
                assert!(s.seen.lock().unwrap().is_empty());
                assert!(!r.failed);
            }
        }
    }

    #[test]
    fn network_protocol_oversized_ids_still_fail_with_oversized_data_urls() {
        let raw = serde_json::to_vec(
            &serde_json::json!({"method":"Page.frameNavigated","params":{
            "frame":{"id":"x".repeat(MAX_ID + 1),"loaderId":"doc",
                "url":format!("data:image/png;base64,{}", "A".repeat(MAX_MESSAGE * 2))}}}),
        )
        .unwrap();
        assert!(parse(&raw).is_err());
    }
}
