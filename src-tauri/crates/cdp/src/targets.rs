//! Browser-related iframe sessions; target IDs are never treated as frame IDs.
use crate::Rejected;
use crate::protocol::{Event, MAX_FRAMES, MAX_ID, Message, Reducer, Sink};
use std::collections::{HashMap, HashSet};

pub const METHODS: [&str; 4] = [
    "Page.enable",
    "Network.enable",
    "Page.getFrameTree",
    "Target.setAutoAttach",
];
pub const MAX_CHILDREN: usize = 32;
const MAX_TARGET_HISTORY: usize = 256;
const MAX_SESSION_HISTORY: usize = 512;
const SETUP_TTL_MS: u64 = 5_000;

pub fn command(session: &str, id: i32, method: &str) -> Vec<u8> {
    let params = if method == "Target.setAutoAttach" {
        serde_json::json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true,
            "filter":[{"type":"iframe","exclude":false},{"exclude":true}]})
    } else if method == "Network.enable" {
        // Publication imports can be large; request bodies are not evidence.
        serde_json::json!({"maxPostDataSize":0})
    } else {
        serde_json::json!({})
    };
    let mut message = serde_json::json!({"id":id,"method":method,"params":params});
    if !session.is_empty() {
        message["sessionId"] = session.into();
    }
    serde_json::to_vec(&message).expect("protocol command serialization")
}

pub fn acknowledge(
    pending: &mut Vec<(i32, &'static str)>,
    id: i32,
    success: bool,
) -> Result<Option<&'static str>, Rejected> {
    let Some(index) = pending.iter().position(|(own_id, _)| *own_id == id) else {
        return Ok(None);
    };
    if !success {
        return Err(Rejected);
    }
    Ok(Some(pending.remove(index).1))
}

struct Target<S> {
    parent: Option<String>,
    target_id: String,
    reader: Option<String>,
    pending: Option<Vec<(i32, &'static str)>>,
    deadline: u64,
    initial_snapshot: bool,
    attached_sequence: u64,
    reducer: Reducer<S>,
}

impl<S> Target<S> {
    fn ready(&self) -> bool {
        self.pending.as_ref().is_some_and(Vec::is_empty)
    }
}

pub struct Targets<S> {
    sessions: HashMap<String, Target<S>>,
    // Tombstones prevent duplicate attachment, detach/reattach and same-origin
    // reader reopen from turning an existing target into a new snapshot owner.
    target_history: HashMap<String, Option<String>>,
    session_history: HashSet<String>,
    reader_origin: String,
    shell: Option<(String, String, Option<String>)>,
    fresh_frames: HashMap<String, (String, u64)>,
    seen_frames: HashSet<String>,
    seen_shell_loaders: HashSet<String>,
    sequence: u64,
    pub failed: bool,
}

impl<S: Sink> Targets<S> {
    pub fn new(now: u64, reader_origin: &str) -> Self {
        let root = Target {
            parent: None,
            target_id: String::new(),
            reader: None,
            pending: None,
            deadline: now.saturating_add(SETUP_TTL_MS),
            initial_snapshot: false,
            attached_sequence: 0,
            reducer: Reducer::default(),
        };
        Self {
            sessions: HashMap::from([(String::new(), root)]),
            target_history: HashMap::new(),
            session_history: HashSet::new(),
            reader_origin: reader_origin.into(),
            shell: None,
            fresh_frames: HashMap::new(),
            seen_frames: HashSet::new(),
            seen_shell_loaders: HashSet::new(),
            sequence: 0,
            failed: false,
        }
    }

    pub fn fail(&mut self) {
        self.sessions.clear();
        self.target_history.clear();
        self.session_history.clear();
        self.shell = None;
        self.fresh_frames.clear();
        self.seen_frames.clear();
        self.seen_shell_loaders.clear();
        self.failed = true;
    }

    pub fn root_ready(&self) -> bool {
        !self.failed && self.sessions.get("").is_some_and(Target::ready)
    }

    pub fn expire(&mut self, now: u64) {
        if self
            .sessions
            .values()
            .any(|target| !target.ready() && now >= target.deadline)
        {
            self.fail();
        }
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.sessions
            .values()
            .filter(|target| !target.ready())
            .map(|target| target.deadline)
            .min()
    }

    pub fn setup(&mut self, session: &str, ids: [i32; 4]) {
        if let Some(target) = self.sessions.get_mut(session)
            && target.pending.is_none()
        {
            target.pending = Some(ids.into_iter().zip(METHODS).collect());
        }
    }

    pub fn pending_replies(&self) -> HashSet<(String, i32)> {
        self.sessions
            .iter()
            .flat_map(|(session, target)| {
                target
                    .pending
                    .iter()
                    .flatten()
                    .map(move |(id, _)| (session.clone(), *id))
            })
            .collect()
    }

    fn detach(&mut self, session: &str) {
        let mut pending = vec![session.to_owned()];
        while let Some(session) = pending.pop() {
            pending.extend(
                self.sessions
                    .iter()
                    .filter(|(_, t)| t.parent.as_deref() == Some(&session))
                    .map(|(id, _)| id.clone()),
            );
            self.sessions.remove(&session);
        }
    }

    /// Returns a new child session requiring setup commands from the controller.
    pub fn receive(
        &mut self,
        message: Message,
        reader: Option<&str>,
        at: u64,
        now: u64,
        bind: impl Fn(&str) -> Option<S>,
    ) -> Option<String> {
        if self.failed {
            return None;
        }
        self.sequence += 1;
        let session = message.session.as_deref().unwrap_or("");
        if session.is_empty()
            && let Event::Navigation {
                frame,
                parent: None,
                loader,
                origin,
                ..
            } = &message.event
        {
            if self
                .shell
                .as_ref()
                .is_some_and(|(_, current, _)| current == loader)
            {
                return None;
            }
            let retired = self.seen_shell_loaders.contains(loader);
            if !retired && self.seen_shell_loaders.len() >= MAX_TARGET_HISTORY {
                self.fail();
                return None;
            }
            self.seen_shell_loaders.insert(loader.clone());
            // Page frame IDs persist across document navigation. Neither old
            // targets nor old frame-attachment records survive a shell commit.
            self.sessions.retain(|id, _| id.is_empty());
            self.sessions.get_mut("")?.reducer = Reducer::default();
            self.fresh_frames.clear();
            // A restored/late old loader is not a newly created shell. Drop
            // provenance rather than keeping that of the document it replaced.
            self.shell = (!retired && origin == &self.reader_origin)
                .then(|| (frame.clone(), loader.clone(), reader.map(str::to_owned)));
        }
        if self.sessions.contains_key(session) {
            match &message.event {
                Event::Attached(frame, parent) => {
                    // A repeated attach cannot make an old browser frame fresh.
                    if !self.seen_frames.contains(frame) {
                        if self.seen_frames.len() >= MAX_SESSION_HISTORY {
                            self.fail();
                            return None;
                        }
                        self.seen_frames.insert(frame.clone());
                        if self.shell.as_ref().is_some_and(|(root, _, owner)| {
                            owner.is_some()
                                && owner.as_deref() == reader
                                && (parent == root || self.fresh_frames.contains_key(parent))
                        }) {
                            if self.fresh_frames.len() >= MAX_FRAMES {
                                self.fail();
                                return None;
                            }
                            self.fresh_frames
                                .insert(frame.clone(), (parent.clone(), self.sequence));
                        }
                    }
                }
                Event::Detached(frame) => {
                    let mut pending = vec![frame.clone()];
                    while let Some(frame) = pending.pop() {
                        pending.extend(
                            self.fresh_frames
                                .iter()
                                .filter(|(_, (parent, _))| parent == &frame)
                                .map(|(id, _)| id.clone()),
                        );
                        self.fresh_frames.remove(&frame);
                    }
                }
                // A process swap removes the local frame, not its browser ancestry.
                Event::FrameSwapped(_) => {}
                _ => {}
            }
        }
        let parent = self.sessions.get(session)?;
        match message.event {
            Event::TargetAttached {
                session: child,
                target,
                parent_frame,
            } => {
                if parent.pending.is_none()
                    || child.is_empty()
                    || child.len() > MAX_ID
                    || target.is_empty()
                    || target.len() > MAX_ID
                {
                    return None;
                }
                if self.session_history.contains(&child)
                    || self.sessions.values().any(|s| s.target_id == target)
                {
                    return None;
                }
                if self.sessions.len() > MAX_CHILDREN
                    || self.session_history.len() >= MAX_SESSION_HISTORY
                    || (!self.target_history.contains_key(&target)
                        && self.target_history.len() >= MAX_TARGET_HISTORY)
                {
                    self.fail();
                    return None;
                }
                let initial_snapshot = !self.target_history.contains_key(&target);
                let captured_reader = if session.is_empty() {
                    reader.map(str::to_owned)
                } else {
                    parent.reader.clone()
                };
                let captured_reader = self
                    .target_history
                    .get(&target)
                    .cloned()
                    .unwrap_or(captured_reader);
                let inherited = parent_frame
                    .and_then(|frame| parent.reducer.owner(&frame).map(|sink| (frame, sink)))
                    .filter(|(_, sink)| Some(sink.session_id()) == captured_reader.as_deref());
                let mut reducer = Reducer::default();
                reducer.set_external_parent(inherited);
                self.target_history
                    .entry(target.clone())
                    .or_insert_with(|| captured_reader.clone());
                self.session_history.insert(child.clone());
                self.sessions.insert(
                    child.clone(),
                    Target {
                        parent: Some(session.to_owned()),
                        target_id: target,
                        reader: captured_reader,
                        pending: None,
                        deadline: now.saturating_add(SETUP_TTL_MS),
                        initial_snapshot,
                        attached_sequence: self.sequence,
                        reducer,
                    },
                );
                Some(child)
            }
            Event::TargetDetached(child) => {
                if self
                    .sessions
                    .get(&child)
                    .is_some_and(|target| target.parent.as_deref() == Some(session))
                {
                    self.detach(&child);
                }
                None
            }
            Event::Ack(id, success, frames) => {
                let provenance = self.sessions.get(session).is_some_and(|target| {
                    frames.first().is_some_and(|frame| {
                        self.shell.as_ref().is_some_and(|(_, _, owner)| {
                            owner.is_some() && owner.as_deref() == reader
                        }) && self
                            .fresh_frames
                            .get(frame.id())
                            .is_some_and(|(_, sequence)| *sequence < target.attached_sequence)
                    })
                });
                let target = self.sessions.get_mut(session)?;
                let pending = target.pending.as_mut()?;
                let method = match acknowledge(pending, id, success) {
                    Ok(Some(method)) => method,
                    Ok(None) => return None,
                    Err(Rejected) => {
                        self.fail();
                        return None;
                    }
                };
                if method == "Page.getFrameTree" {
                    if target.initial_snapshot {
                        target.initial_snapshot = false;
                        // Receipt is not creation: resolve the snapshot's canonical
                        // Page frame ID to a fresh attach in this shell document.
                        if provenance && reader == target.reader.as_deref() {
                            let bound = target.reducer.initial_child_snapshot(
                                frames,
                                target.reader.as_deref(),
                                &bind,
                            );
                            tracing::debug!(
                                event = "network_observer_initial_child_snapshot",
                                bound
                            );
                        } else {
                            target
                                .reducer
                                .apply(Event::Snapshot(frames), None, 0, |_| None);
                        }
                    } else {
                        target
                            .reducer
                            .apply(Event::Snapshot(frames), None, 0, |_| None);
                    }
                }
                if target.reducer.failed {
                    self.fail();
                }
                None
            }
            event => {
                let target = self.sessions.get_mut(session)?;
                if !session.is_empty() && reader != target.reader.as_deref() {
                    return None;
                }
                if target.ready()
                    || matches!(
                        event,
                        Event::Navigation { .. }
                            | Event::Attached(..)
                            | Event::Detached(..)
                            | Event::FrameSwapped(..)
                            | Event::Disconnected
                    )
                {
                    target.reducer.apply(event, reader, at, bind);
                }
                if target.reducer.failed {
                    self.fail();
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    const SHELL: &str = "http://reader.localhost:4003";

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
                seen: Arc::default(),
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
    fn deliver(
        t: &mut Targets<TestSink>,
        session: &str,
        event: Event,
        reader: &str,
        sink: &TestSink,
    ) -> Option<String> {
        t.receive(
            Message {
                session: (!session.is_empty()).then(|| session.into()),
                event,
            },
            Some(reader),
            42,
            0,
            |origin| {
                (origin == "http://p-book.localhost:4000"
                    || origin == "http://n-book.localhost:4001")
                    .then(|| sink.clone())
            },
        )
    }
    fn tree(id: i32, frame: &str, origin: &str) -> Event {
        parse(
            &serde_json::to_vec(&serde_json::json!({"id":id,"result":{"frameTree":{"frame":{
            "id":frame,"loaderId":"doc","url":format!("{origin}/private?secret")}}}}))
            .unwrap(),
        )
        .unwrap()
        .event
    }
    fn root(s: &TestSink) -> Targets<TestSink> {
        let mut targets = Targets::new(0, SHELL);
        targets.setup("", [1, 2, 3, 4]);
        for id in [1, 2, 3, 4] {
            deliver(&mut targets, "", Event::Ack(id, true, vec![]), s.id, s);
        }
        shell(&mut targets, "shell-doc", s);
        targets
    }
    fn shell(t: &mut Targets<TestSink>, loader: &str, s: &TestSink) {
        deliver(
            t,
            "",
            Event::Navigation {
                frame: "shell-frame".into(),
                parent: None,
                loader: loader.into(),
                origin: SHELL.into(),
                inheritable: false,
            },
            s.id,
            s,
        );
    }
    fn fresh(t: &mut Targets<TestSink>, session: &str, frame: &str, parent: &str, s: &TestSink) {
        deliver(
            t,
            session,
            Event::Attached(frame.into(), parent.into()),
            s.id,
            s,
        );
    }
    fn attach(
        t: &mut Targets<TestSink>,
        parent: &str,
        session: &str,
        target: &str,
        frame: Option<&str>,
        s: &TestSink,
    ) -> Option<String> {
        deliver(
            t,
            parent,
            Event::TargetAttached {
                session: session.into(),
                target: target.into(),
                parent_frame: frame.map(str::to_owned),
            },
            s.id,
            s,
        )
    }
    fn ready(t: &mut Targets<TestSink>, session: &str, frame: &str, origin: &str, s: &TestSink) {
        t.setup(session, [11, 12, 13, 14]);
        for id in [11, 12, 14] {
            deliver(t, session, Event::Ack(id, true, vec![]), s.id, s);
        }
        deliver(t, session, tree(13, frame, origin), s.id, s);
    }
    fn traffic(t: &mut Targets<TestSink>, session: &str, frame: &str, loader: &str, s: &TestSink) {
        deliver(
            t,
            session,
            Event::Request {
                id: "request".into(),
                frame: frame.into(),
                loader: loader.into(),
                remote: true,
                redirect: false,
            },
            s.id,
            s,
        );
        deliver(t, session, Event::Response("request".into(), true), s.id, s);
    }

    #[test]
    fn network_targets_root_requires_autoattach_and_commands_are_iframe_only() {
        let s = TestSink::new("reader");
        let mut t = Targets::new(0, SHELL);
        t.setup("", [1, 2, 3, 4]);
        for id in [1, 2, 3] {
            deliver(&mut t, "", Event::Ack(id, true, vec![]), s.id, &s);
        }
        assert!(!t.root_ready());
        deliver(&mut t, "", Event::Ack(999, true, vec![]), s.id, &s);
        assert!(!t.root_ready());
        deliver(&mut t, "", Event::Ack(4, true, vec![]), s.id, &s);
        assert!(t.root_ready());
        let value: serde_json::Value =
            serde_json::from_slice(&command("child", 17, METHODS[3])).unwrap();
        assert_eq!(value["sessionId"], "child");
        let network: serde_json::Value =
            serde_json::from_slice(&command("child", 18, "Network.enable")).unwrap();
        assert_eq!(network["params"]["maxPostDataSize"], 0);
        assert_eq!(
            value["params"],
            serde_json::json!({"autoAttach":true,"flatten":true,"waitForDebuggerOnStart":false,
            "filter":[{"type":"iframe","exclude":false},{"exclude":true}]})
        );
    }

    #[test]
    fn network_targets_child_snapshot_binds_once_after_own_setup() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "actual-frame-id", "shell-frame", &s);
        deliver(
            &mut t,
            "",
            Event::FrameSwapped("actual-frame-id".into()),
            s.id,
            &s,
        );
        assert_eq!(
            attach(&mut t, "", "child", "not-a-frame-id", None, &s).as_deref(),
            Some("child")
        );
        t.setup("child", [11, 12, 13, 14]);
        deliver(
            &mut t,
            "child",
            tree(13, "actual-frame-id", "http://p-book.localhost:4000"),
            s.id,
            &s,
        );
        traffic(&mut t, "child", "actual-frame-id", "doc", &s);
        assert!(s.seen.lock().unwrap().is_empty());
        for id in [11, 12, 14] {
            deliver(&mut t, "child", Event::Ack(id, true, vec![]), s.id, &s);
        }
        traffic(&mut t, "child", "actual-frame-id", "doc", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
        assert!(
            t.sessions["child"]
                .reducer
                .owner("not-a-frame-id")
                .is_none()
        );
    }

    #[test]
    fn network_targets_stale_attach_and_repeat_snapshot_cannot_rebind_reopened_reader() {
        let old = TestSink::new("old");
        let new = TestSink::new("new");
        let mut t = root(&old);
        fresh(&mut t, "", "frame", "shell-frame", &old);
        attach(&mut t, "", "child", "target", None, &old);
        old.active.store(false, Ordering::Release);
        ready(
            &mut t,
            "child",
            "frame",
            "http://p-book.localhost:4000",
            &new,
        );
        deliver(
            &mut t,
            "child",
            tree(13, "frame", "http://p-book.localhost:4000"),
            new.id,
            &new,
        );
        traffic(&mut t, "child", "frame", "doc", &new);
        assert!(new.seen.lock().unwrap().is_empty());
        assert!(attach(&mut t, "", "repeat", "target", None, &new).is_none());
        deliver(
            &mut t,
            "",
            Event::TargetDetached("child".into()),
            new.id,
            &new,
        );
        attach(&mut t, "", "reattach", "target", None, &new);
        ready(
            &mut t,
            "reattach",
            "frame",
            "http://p-book.localhost:4000",
            &new,
        );
        traffic(&mut t, "reattach", "frame", "doc", &new);
        assert!(new.seen.lock().unwrap().is_empty());
        assert_eq!(t.sessions["reattach"].reader.as_deref(), Some("old"));
    }

    #[test]
    fn network_targets_unknown_origin_waits_for_fresh_native_commit() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "frame", "shell-frame", &s);
        attach(&mut t, "", "child", "target", None, &s);
        ready(&mut t, "child", "frame", "https://unowned.example", &s);
        traffic(&mut t, "child", "frame", "doc", &s);
        assert!(s.seen.lock().unwrap().is_empty());
        deliver(
            &mut t,
            "child",
            Event::Navigation {
                frame: "frame".into(),
                parent: None,
                loader: "fresh".into(),
                origin: "http://p-book.localhost:4000".into(),
                inheritable: false,
            },
            s.id,
            &s,
        );
        traffic(&mut t, "child", "frame", "fresh", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
    }

    #[test]
    fn network_targets_root_snapshot_and_colliding_namespaces_do_not_authorize_root_traffic() {
        let s = TestSink::new("reader");
        let mut t = Targets::new(0, SHELL);
        t.setup("", [1, 2, 3, 4]);
        shell(&mut t, "shell-doc", &s);
        deliver(
            &mut t,
            "",
            tree(3, "frame", "http://p-book.localhost:4000"),
            s.id,
            &s,
        );
        for id in [1, 2, 4] {
            deliver(&mut t, "", Event::Ack(id, true, vec![]), s.id, &s);
        }
        fresh(&mut t, "", "frame", "shell-frame", &s);
        attach(&mut t, "", "child", "target", None, &s);
        ready(&mut t, "child", "frame", "http://p-book.localhost:4000", &s);
        traffic(&mut t, "", "frame", "doc", &s);
        traffic(&mut t, "unknown", "frame", "doc", &s);
        assert!(s.seen.lock().unwrap().is_empty());
        traffic(&mut t, "child", "frame", "doc", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
    }

    #[test]
    fn network_targets_acknowledgements_cannot_cross_child_sessions() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        attach(&mut t, "", "a", "ta", None, &s);
        attach(&mut t, "", "b", "tb", None, &s);
        t.setup("a", [11, 12, 13, 14]);
        t.setup("b", [21, 22, 23, 24]);
        deliver(&mut t, "b", Event::Ack(12, false, vec![]), s.id, &s);
        assert!(!t.failed);
        assert_eq!(t.sessions["a"].pending.as_ref().unwrap().len(), 4);
        deliver(&mut t, "a", Event::Ack(12, false, vec![]), s.id, &s);
        assert!(t.failed && !t.root_ready());
    }

    #[test]
    fn network_targets_recursive_remote_children_require_proven_parent_frame() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "publication-frame", "shell-frame", &s);
        attach(&mut t, "", "child", "target", None, &s);
        ready(
            &mut t,
            "child",
            "publication-frame",
            "http://p-book.localhost:4000",
            &s,
        );
        fresh(&mut t, "child", "remote-frame", "publication-frame", &s);
        attach(
            &mut t,
            "child",
            "nested",
            "nested-target",
            Some("publication-frame"),
            &s,
        );
        ready(
            &mut t,
            "nested",
            "remote-frame",
            "https://remote.example",
            &s,
        );
        attach(&mut t, "child", "unknown", "unknown-target", None, &s);
        ready(
            &mut t,
            "unknown",
            "remote-frame",
            "https://remote.example",
            &s,
        );
        traffic(&mut t, "nested", "remote-frame", "doc", &s);
        traffic(&mut t, "unknown", "remote-frame", "doc", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
        deliver(&mut t, "", Event::TargetDetached("nested".into()), s.id, &s);
        assert!(t.sessions.contains_key("nested"));
        deliver(&mut t, "", Event::TargetDetached("child".into()), s.id, &s);
        assert_eq!(t.sessions.len(), 1);
        traffic(&mut t, "nested", "remote-frame", "doc", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
    }

    #[test]
    fn network_targets_measurement_and_unrelated_parent_never_authorize() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "frame", "shell-frame", &s);
        assert!(attach(&mut t, "popup", "child", "target", None, &s).is_none());
        attach(&mut t, "", "measurement", "m", None, &s);
        ready(
            &mut t,
            "measurement",
            "frame",
            "http://m-book.localhost:4002",
            &s,
        );
        traffic(&mut t, "measurement", "frame", "doc", &s);
        assert!(s.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn network_targets_child_timeout_and_capacity_fail_closed() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        attach(&mut t, "", "child", "target", None, &s);
        t.expire(SETUP_TTL_MS - 1);
        assert!(!t.failed);
        t.expire(SETUP_TTL_MS);
        assert!(t.failed && t.sessions.is_empty());
        let mut t = root(&s);
        for i in 0..MAX_CHILDREN {
            assert!(attach(&mut t, "", &format!("s{i}"), &format!("t{i}"), None, &s).is_some());
        }
        assert!(!t.failed);
        attach(&mut t, "", "overflow", "overflow", None, &s);
        assert!(t.failed);
        assert!(attach(&mut t, "", "later", "later", None, &s).is_none());
    }

    #[test]
    fn network_targets_history_is_bounded_across_detach() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        for i in 0..MAX_TARGET_HISTORY {
            let child = format!("s{i}");
            attach(&mut t, "", &child, &format!("t{i}"), None, &s);
            deliver(&mut t, "", Event::TargetDetached(child), s.id, &s);
        }
        assert!(!t.failed);
        attach(&mut t, "", "overflow", "overflow", None, &s);
        assert!(t.failed);
        let mut t = root(&s);
        for i in 0..MAX_SESSION_HISTORY {
            let child = format!("s{i}");
            attach(&mut t, "", &child, "same-target", None, &s);
            deliver(&mut t, "", Event::TargetDetached(child), s.id, &s);
        }
        assert!(!t.failed);
        attach(&mut t, "", "overflow", "same-target", None, &s);
        assert!(t.failed);
    }

    #[test]
    fn network_targets_parser_preserves_outer_session_rejects_workers_and_bounds_ids() {
        let raw = br#"{"sessionId":"parent","method":"Target.attachedToTarget","params":{"sessionId":"child","targetInfo":{"type":"iframe","targetId":"target","url":"http://p-book.localhost/private"}}}"#;
        let message = parse(raw).unwrap();
        assert_eq!(message.session.as_deref(), Some("parent"));
        assert!(
            matches!(message.event, Event::TargetAttached { session, target, .. } if session == "child" && target == "target")
        );
        let worker = br#"{"method":"Target.attachedToTarget","params":{"sessionId":"worker","targetInfo":{"type":"worker","targetId":"w"}}}"#;
        assert!(matches!(parse(worker).unwrap().event, Event::Ignore));
        for size in [MAX_ID, MAX_ID + 1] {
            let raw = serde_json::to_vec(
                &serde_json::json!({"sessionId":"s".repeat(size),"id":1,"result":{}}),
            )
            .unwrap();
            assert_eq!(parse(&raw).is_ok(), size == MAX_ID);
            let raw = serde_json::to_vec(
                &serde_json::json!({"method":"Target.attachedToTarget","params":{
                "sessionId":"s","targetInfo":{"type":"iframe","targetId":"t".repeat(size)}}}),
            )
            .unwrap();
            assert_eq!(parse(&raw).is_ok(), size == MAX_ID);
        }
    }

    #[test]
    fn network_targets_initial_tree_never_overwrites_a_fresh_commit() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "frame", "shell-frame", &s);
        attach(&mut t, "", "child", "target", None, &s);
        t.setup("child", [11, 12, 13, 14]);
        deliver(
            &mut t,
            "child",
            Event::Navigation {
                frame: "frame".into(),
                parent: None,
                loader: "fresh".into(),
                origin: "http://p-book.localhost:4000".into(),
                inheritable: false,
            },
            s.id,
            &s,
        );
        for id in [11, 12, 14] {
            deliver(&mut t, "child", Event::Ack(id, true, vec![]), s.id, &s);
        }
        deliver(
            &mut t,
            "child",
            tree(13, "frame", "https://unowned.example"),
            s.id,
            &s,
        );
        traffic(&mut t, "child", "frame", "fresh", &s);
        assert_eq!(*s.seen.lock().unwrap(), [42]);
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn network_targets_first_late_old_attachment_cannot_bind_replacement_reader() {
        for commit_new_shell in [false, true] {
            let old = TestSink::new("old");
            let new = TestSink::new("new");
            let mut t = root(&old);
            fresh(&mut t, "", "old-frame", "shell-frame", &old);
            old.active.store(false, Ordering::Release);
            if commit_new_shell {
                shell(&mut t, "replacement-shell", &new);
            }
            // The parent frame ID is unchanged across shell documents. Neither
            // that ID, the new receipt session, nor a different fresh frame proves
            // creation of this previously unseen target's actual document.
            fresh(&mut t, "", "different-fresh-frame", "shell-frame", &new);
            fresh(&mut t, "", "old-frame", "shell-frame", &new);
            assert!(!t.target_history.contains_key("different-fresh-frame"));
            attach(
                &mut t,
                "",
                "late-old",
                "different-fresh-frame",
                Some("shell-frame"),
                &new,
            );
            ready(
                &mut t,
                "late-old",
                "old-frame",
                "http://p-book.localhost:4000",
                &new,
            );
            traffic(&mut t, "late-old", "old-frame", "doc", &new);
            assert!(new.seen.lock().unwrap().is_empty());
            // A genuinely new browser frame in the new shell still binds from
            // its initial snapshot without needing a child frameNavigated event.
            if !commit_new_shell {
                shell(&mut t, "replacement-shell", &new);
            }
            fresh(&mut t, "", "new-frame", "shell-frame", &new);
            attach(&mut t, "", "new-child", "new-target", None, &new);
            ready(
                &mut t,
                "new-child",
                "new-frame",
                "http://p-book.localhost:4000",
                &new,
            );
            traffic(&mut t, "new-child", "new-frame", "doc", &new);
            assert_eq!(*new.seen.lock().unwrap(), [42]);
        }
    }

    #[test]
    fn network_targets_snapshot_proof_must_predate_target_and_survive_until_reply() {
        for removed in [false, true] {
            let s = TestSink::new("reader");
            let mut t = root(&s);
            if removed {
                fresh(&mut t, "", "frame", "shell-frame", &s);
            }
            attach(&mut t, "", "child", "target", None, &s);
            if removed {
                deliver(&mut t, "", Event::Detached("frame".into()), s.id, &s);
            } else {
                fresh(&mut t, "", "frame", "shell-frame", &s);
            }
            ready(&mut t, "child", "frame", "http://p-book.localhost:4000", &s);
            traffic(&mut t, "child", "frame", "doc", &s);
            assert!(s.seen.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn network_targets_shell_commit_retires_child_graph_and_pending_replies() {
        let s = TestSink::new("reader");
        let mut t = root(&s);
        fresh(&mut t, "", "frame", "shell-frame", &s);
        attach(&mut t, "", "child", "target", None, &s);
        t.setup("child", [11, 12, 13, 14]);
        assert_eq!(t.pending_replies().len(), 4);
        shell(&mut t, "next-document", &s);
        assert_eq!(t.sessions.len(), 1);
        assert!(t.pending_replies().is_empty());
        assert!(t.fresh_frames.is_empty());
        deliver(
            &mut t,
            "child",
            tree(13, "frame", "http://p-book.localhost:4000"),
            s.id,
            &s,
        );
        traffic(&mut t, "child", "frame", "doc", &s);
        shell(&mut t, "shell-doc", &s); // Late duplicate old commit must not revive it.
        assert!(t.shell.is_none());
        fresh(&mut t, "", "restored-child", "shell-frame", &s);
        assert!(t.fresh_frames.is_empty());
        assert!(s.seen.lock().unwrap().is_empty());
        assert!(!t.failed);
    }
}
