use elephant_ladder_cdp::protocol::{self as network_protocol, Event, Message, Sink};
#[cfg(test)]
use elephant_ladder_cdp::targets::acknowledge;
use elephant_ladder_cdp::targets::{self as network_targets, METHODS, Targets};
use elephant_ladder_native_host::{NetworkActivityObserver, SessionNetworkActivitySink};
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri_runtime_cef::{DevToolsProtocol, WebviewCefExt, allocate_devtools_message_id};

// At most 64 * 64 KiB of string metadata queued; queued events contain only
// IDs, origins and booleans, never raw protocol messages or network URLs.
const QUEUE_LENGTH: usize = 64;
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(5);

// Shared with the navigation policy, but contains no window or reader URL.
// Pending allows only the inert bootstrap; stopped also rejects queued launches.
#[derive(Default)]
pub struct Bootstrap(AtomicU8);

impl Bootstrap {
    pub fn allows(&self, url: &tauri::Url, normal_policy: bool) -> bool {
        match self.0.load(Ordering::Acquire) {
            0 => url.as_str() == "about:blank",
            1 => normal_policy,
            _ => false,
        }
    }

    fn stop(&self) {
        self.0.store(2, Ordering::Release);
    }
}

/// Adapts the host's activity sink to the protocol crate's.
///
/// The protocol crate knows nothing about reader sessions and the host knows
/// nothing about CDP, so neither can own this impl; the platform glue does.
#[derive(Clone)]
struct ActivitySink(SessionNetworkActivitySink);

impl Sink for ActivitySink {
    fn session_id(&self) -> &str {
        self.0.session_id()
    }
    fn active(&self) -> bool {
        self.0.is_active()
    }
    fn observe(&self, at: u64) {
        self.0.observe_at(at);
    }
}

struct Browser {
    live: AtomicBool,
    overflow: AtomicBool,
    bootstrap: Arc<Bootstrap>,
    clock: std::time::Instant,
    setup_deadline: AtomicU64,
    replies: Mutex<HashSet<(String, i32)>>,
}

// `launch` may only enqueue navigation, never wait for a CEF/UI response. Holding
// the generation lock makes both launch ownership and its enqueue atomic with stop.
fn launch_current(
    current: &Mutex<Option<Arc<Browser>>>,
    browser: &Arc<Browser>,
    ready: bool,
    launch: impl FnOnce(bool),
) {
    let current = current.lock().unwrap();
    if current.as_ref().is_some_and(|b| Arc::ptr_eq(b, browser))
        && browser.live.load(Ordering::Acquire)
        && browser
            .bootstrap
            .0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        if !ready {
            browser.overflow.store(true, Ordering::Release);
        }
        launch(ready && !browser.overflow.load(Ordering::Acquire));
    }
}

fn publish_current(
    current: &Mutex<Option<Arc<Browser>>>,
    browser: &Arc<Browser>,
    available: bool,
    publish: impl FnOnce(bool),
) {
    let current = current.lock().unwrap();
    if current.as_ref().is_some_and(|b| Arc::ptr_eq(b, browser)) {
        publish(
            available
                && browser.live.load(Ordering::Acquire)
                && !browser.overflow.load(Ordering::Acquire),
        );
    }
}

type Received = (Message, Option<String>, u64);

fn enqueue(
    browser: &Browser,
    tx: &mpsc::SyncSender<Received>,
    raw: &[u8],
    session: Option<String>,
    at: u64,
) {
    if !browser.live.load(Ordering::Acquire) || browser.overflow.load(Ordering::Acquire) {
        return;
    }
    match network_protocol::parse_owned(raw, |cdp_session, id| {
        browser
            .replies
            .lock()
            .unwrap()
            .contains(&(cdp_session.to_owned(), id))
    }) {
        Ok(Message {
            event: Event::Ignore,
            ..
        }) => {}
        Ok(event) => {
            if tx.try_send((event, session, at)).is_err() {
                browser.overflow.store(true, Ordering::Release);
            }
        }
        Err(elephant_ladder_cdp::Rejected) => {
            browser.overflow.store(true, Ordering::Release);
        }
    }
}

/// The mutex serializes generation replacement with availability publication.
/// Neither the callback nor the managed state owns a webview.
#[derive(Clone)]
pub struct Observer {
    host: NetworkActivityObserver,
    current: Arc<Mutex<Option<Arc<Browser>>>>,
}

impl Observer {
    pub fn new(host: NetworkActivityObserver) -> Self {
        host.set_available(false);
        Self {
            host,
            current: Arc::new(Mutex::new(None)),
        }
    }

    pub fn stop(&self) {
        let mut current = self.current.lock().unwrap();
        if let Some(browser) = current.take() {
            browser.live.store(false, Ordering::Release);
            browser.bootstrap.stop();
        }
        self.host.set_available(false);
    }

    fn publish(&self, browser: &Arc<Browser>, available: bool) {
        publish_current(&self.current, browser, available, |value| {
            self.host.set_available(value)
        });
    }

    fn launch(
        &self,
        browser: &Arc<Browser>,
        window: tauri::WebviewWindow,
        url: tauri::Url,
        ready: bool,
    ) {
        launch_current(&self.current, browser, ready, |available| {
            self.host.set_available(available);
            // The pinned CEF navigate implementation enqueues without recv/wait.
            if window.navigate(url).is_err() {
                browser.overflow.store(true, Ordering::Release);
                self.host.set_available(false);
                tracing::error!(event = "reader_bootstrap_navigation_failed");
            }
        });
    }

    pub fn attach(
        &self,
        window: tauri::WebviewWindow,
        reader_url: tauri::Url,
        bootstrap: Arc<Bootstrap>,
    ) {
        let browser = Arc::new(Browser {
            live: AtomicBool::new(true),
            overflow: AtomicBool::new(false),
            bootstrap,
            clock: std::time::Instant::now(),
            setup_deadline: AtomicU64::new(0),
            replies: Mutex::new(HashSet::new()),
        });
        {
            let mut current = self.current.lock().unwrap();
            if let Some(old) = current.replace(browser.clone()) {
                old.live.store(false, Ordering::Release);
                old.bootstrap.stop();
            }
            self.host.set_available(false);
        }
        let owner = self.clone();
        let lifetime = browser.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                lifetime.live.store(false, Ordering::Release);
                lifetime.bootstrap.stop();
                owner.publish(&lifetime, false);
            }
        });
        // Registration/send dispatches themselves have unbounded recv() calls in
        // the pinned runtime. An independent native watchdog covers those too.
        let (launch_done, wait_for_launch) = mpsc::channel::<()>();
        let timeout_owner = self.clone();
        let timeout_browser = browser.clone();
        let timeout_window = window.clone();
        let timeout_url = reader_url.clone();
        if std::thread::Builder::new()
            .name("reader-bootstrap-watchdog".into())
            .spawn(move || {
                let _ = wait_for_launch.recv_timeout(
                    BOOTSTRAP_TIMEOUT.saturating_sub(timeout_browser.clock.elapsed()),
                );
                timeout_owner.launch(&timeout_browser, timeout_window, timeout_url, false);
                // Keep only lightweight lifetime state after bootstrap. CDP send
                // itself can wait inside CEF, so child setup TTL cannot depend on
                // the controller returning to its receive loop.
                while timeout_browser.live.load(Ordering::Acquire)
                    && !timeout_browser.overflow.load(Ordering::Acquire)
                {
                    let deadline = timeout_browser.setup_deadline.load(Ordering::Acquire);
                    if deadline != 0
                        && timeout_browser.clock.elapsed().as_millis() >= deadline as u128
                    {
                        timeout_browser.overflow.store(true, Ordering::Release);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                timeout_owner.publish(&timeout_browser, false);
            })
            .is_err()
        {
            // Even thread exhaustion must not strand a live window on about:blank.
            // navigate is nonblocking on the pinned runtime, unlike CDP dispatch.
            self.launch(&browser, window, reader_url, false);
            return;
        }
        let owner = self.clone();
        if std::thread::Builder::new()
            .name("linux-network-observer".into())
            .spawn(move || {
                owner.control(window, reader_url, browser, launch_done);
            })
            .is_err()
        {
            // Dropping the failed closure disconnects launch_done. The watchdog
            // immediately launches without observation rather than waiting 5 s.
            tracing::warn!(event = "network_observer_thread_unavailable");
        }
    }

    fn control(
        &self,
        window: tauri::WebviewWindow,
        reader_url: tauri::Url,
        browser: Arc<Browser>,
        launch_done: mpsc::Sender<()>,
    ) {
        if !browser.live.load(Ordering::Acquire) || browser.overflow.load(Ordering::Acquire) {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(QUEUE_LENGTH);
        let callback_browser = browser.clone();
        let host = self.host.clone();
        // Register exactly once on this browser. dispatch waits, so all registration
        // and sends happen on this controller, never in a CEF/window callback.
        let registered = window
            .on_dev_tools_protocol(move |message| {
                if !callback_browser.live.load(Ordering::Acquire)
                    || callback_browser.overflow.load(Ordering::Acquire)
                {
                    return;
                }
                let DevToolsProtocol::Message(raw) = message else {
                    return;
                };
                let session = host.current_session_id();
                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u64::MAX as u128) as u64;
                enqueue(&callback_browser, &tx, &raw, session, at);
            })
            .is_ok();
        let mut targets = Targets::<ActivitySink>::new(
            browser.clock.elapsed().as_millis() as u64,
            &reader_url.origin().ascii_serialization(),
        );
        if !registered || !send_setup(&window, &browser, &mut targets, "") {
            targets.fail();
        }
        let mut launch = Some((reader_url, launch_done));
        // The controller retains a dispatch handle for dynamically attached child
        // sessions. Callbacks/managed state never retain the window; Destroyed
        // ends this loop, so there is no window -> callback -> window cycle.
        while !targets.failed && browser.live.load(Ordering::Acquire) {
            if browser.overflow.load(Ordering::Acquire) {
                break;
            }
            let now = browser.clock.elapsed().as_millis().min(u64::MAX as u128) as u64;
            targets.expire(now);
            if targets.failed {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok((message, session, at)) => {
                    if !browser.live.load(Ordering::Acquire)
                        || browser.overflow.load(Ordering::Acquire)
                    {
                        break;
                    }
                    // No dispatch while holding this generation fence. It also
                    // prevents an old controller recording evidence after replacement.
                    let child = {
                        let current = self.current.lock().unwrap();
                        if !current.as_ref().is_some_and(|b| Arc::ptr_eq(b, &browser)) {
                            break;
                        }
                        targets.receive(message, session.as_deref(), at, now, |origin| {
                            self.host.bind_frame_origin(origin).map(ActivitySink)
                        })
                    };
                    *browser.replies.lock().unwrap() = targets.pending_replies();
                    browser
                        .setup_deadline
                        .store(targets.next_deadline().unwrap_or(0), Ordering::Release);
                    if let Some(child) = child
                        && !send_setup(&window, &browser, &mut targets, &child)
                    {
                        targets.fail();
                    }
                    if targets.root_ready()
                        && let Some((url, done)) = launch.take()
                    {
                        self.launch(&browser, window.clone(), url, true);
                        drop(done);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        targets.fail();
        browser.replies.lock().unwrap().clear();
        if let Some((url, done)) = launch.take() {
            self.launch(&browser, window.clone(), url, false);
            drop(done);
        }
        browser.overflow.store(true, Ordering::Release);
        self.publish(&browser, false);
    }
}

fn send_setup(
    window: &tauri::WebviewWindow,
    browser: &Browser,
    targets: &mut Targets<ActivitySink>,
    session: &str,
) -> bool {
    let mut ids = [0; 4];
    for id in &mut ids {
        let Ok(next) = allocate_devtools_message_id() else {
            return false;
        };
        *id = next;
    }
    targets.setup(session, ids);
    *browser.replies.lock().unwrap() = targets.pending_replies();
    browser
        .setup_deadline
        .store(targets.next_deadline().unwrap_or(0), Ordering::Release);
    for (id, method) in ids.into_iter().zip(METHODS) {
        if !browser.live.load(Ordering::Acquire) || browser.overflow.load(Ordering::Acquire) {
            return false;
        }
        if window
            .send_dev_tools_message(&network_targets::command(session, id, method))
            .is_err()
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser() -> Arc<Browser> {
        Arc::new(Browser {
            live: AtomicBool::new(true),
            overflow: AtomicBool::new(false),
            bootstrap: Arc::new(Bootstrap::default()),
            clock: std::time::Instant::now(),
            setup_deadline: AtomicU64::new(0),
            replies: Mutex::new(HashSet::new()),
        })
    }

    #[test]
    fn network_observer_generation_fences_old_enable_and_disable() {
        let old = browser();
        let new = browser();
        let current = Mutex::new(Some(new.clone()));
        publish_current(&current, &old, true, |_| panic!("old enable"));
        publish_current(&current, &old, false, |_| panic!("old disable"));
        publish_current(&current, &new, true, |value| assert!(value));
        new.live.store(false, Ordering::Release);
        publish_current(&current, &new, true, |value| assert!(!value));
        *current.lock().unwrap() = None;
        publish_current(&current, &new, true, |_| panic!("stopped generation"));
    }

    #[test]
    fn network_observer_bootstrap_launches_once_after_all_acknowledgements() {
        let browser = browser();
        let current = Mutex::new(Some(browser.clone()));
        let blank = tauri::Url::parse("about:blank").unwrap();
        let reader = tauri::Url::parse("http://reader.elephant-ladder.localhost:1234/").unwrap();
        assert!(browser.bootstrap.allows(&blank, false));
        assert!(!browser.bootstrap.allows(&reader, true));
        assert!(
            !browser
                .bootstrap
                .allows(&tauri::Url::parse("about:blank#extra").unwrap(), false)
        );
        let mut pending = vec![
            (1, "Page.enable"),
            (2, "Network.enable"),
            (3, "Page.getFrameTree"),
            (4, "Target.setAutoAttach"),
        ];
        let mut launches = 0;
        for id in [1, 3, 2, 4] {
            acknowledge(&mut pending, id, true).unwrap();
            if pending.is_empty() {
                launch_current(&current, &browser, true, |ready| {
                    assert!(ready);
                    launches += 1;
                });
            } else {
                assert_eq!(launches, 0);
            }
        }
        launch_current(&current, &browser, false, |_| {
            panic!("watchdog repeated launch")
        });
        assert_eq!(launches, 1);
        assert!(browser.bootstrap.allows(&reader, true));
        assert!(!browser.bootstrap.allows(&blank, false));
        assert!(!browser.bootstrap.allows(&reader, false));
    }

    #[test]
    fn network_observer_bootstrap_failure_timeout_and_late_ack_launch_only_once() {
        // All failure exits, including dispatch failure/timeout, share this gate.
        for overflow in [false, true] {
            let browser = browser();
            browser.overflow.store(overflow, Ordering::Release);
            let current = Mutex::new(Some(browser.clone()));
            let mut launches = 0;
            launch_current(&current, &browser, false, |ready| {
                assert!(!ready);
                launches += 1;
            });
            launch_current(&current, &browser, true, |_| {
                panic!("late acknowledgement launched")
            });
            assert_eq!(launches, 1);
            assert!(browser.overflow.load(Ordering::Acquire));
            assert!(browser.bootstrap.allows(
                &tauri::Url::parse("http://reader.localhost/").unwrap(),
                true
            ));
        }
    }

    #[test]
    fn network_observer_bootstrap_stopped_and_replaced_browsers_cannot_launch() {
        let old = browser();
        let new = browser();
        let current = Mutex::new(Some(new.clone()));
        for ready in [false, true] {
            launch_current(&current, &old, ready, |_| panic!("replaced launch"));
        }
        new.live.store(false, Ordering::Release);
        new.bootstrap.stop();
        for ready in [false, true] {
            launch_current(&current, &new, ready, |_| panic!("stopped launch"));
        }
        assert!(
            !new.bootstrap
                .allows(&tauri::Url::parse("about:blank").unwrap(), true)
        );
        assert!(!new.bootstrap.allows(
            &tauri::Url::parse("http://reader.localhost/").unwrap(),
            true
        ));
    }

    #[test]
    fn network_observer_bootstrap_stop_rejects_already_enqueued_navigation() {
        let browser = browser();
        let current = Mutex::new(Some(browser.clone()));
        launch_current(&current, &browser, true, |_| {});
        browser.bootstrap.stop();
        assert!(!browser.bootstrap.allows(
            &tauri::Url::parse("http://reader.localhost/").unwrap(),
            true
        ));
    }

    #[test]
    fn network_observer_bootstrap_controller_and_watchdog_race_launches_once() {
        let browser = browser();
        let current = Mutex::new(Some(browser.clone()));
        let launches = std::sync::atomic::AtomicUsize::new(0);
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for ready in [true, false] {
                let browser = &browser;
                let current = &current;
                let launches = &launches;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    launch_current(current, browser, ready, |_| {
                        launches.fetch_add(1, Ordering::AcqRel);
                    })
                });
            }
        });
        assert_eq!(launches.load(Ordering::Acquire), 1);
    }

    #[test]
    fn network_observer_requires_all_own_acknowledgements_including_network() {
        let mut pending = vec![
            (11, "Page.enable"),
            (12, "Network.enable"),
            (13, "Page.getFrameTree"),
            (14, "Target.setAutoAttach"),
        ];
        assert_eq!(acknowledge(&mut pending, 999, false), Ok(None));
        assert_eq!(pending.len(), 4);
        assert_eq!(acknowledge(&mut pending, 11, true), Ok(Some("Page.enable")));
        assert_eq!(
            acknowledge(&mut pending, 13, true),
            Ok(Some("Page.getFrameTree"))
        );
        assert_eq!(
            acknowledge(&mut pending, 14, true),
            Ok(Some("Target.setAutoAttach"))
        );
        assert_eq!(pending, [(12, "Network.enable")]);
        assert_eq!(
            acknowledge(&mut pending, 12, false),
            Err(elephant_ladder_cdp::Rejected)
        );
        assert!(!pending.is_empty());
        assert_eq!(
            acknowledge(&mut pending, 12, true),
            Ok(Some("Network.enable"))
        );
        assert!(pending.is_empty());
        assert_eq!(acknowledge(&mut pending, 12, true), Ok(None));
    }

    #[test]
    fn network_observer_queue_is_bounded_and_overflow_is_terminal() {
        let b = browser();
        b.replies.lock().unwrap().insert((String::new(), 1));
        let (tx, rx) = mpsc::sync_channel(QUEUE_LENGTH);
        let raw = br#"{"id":1,"result":{}}"#;
        for _ in 0..QUEUE_LENGTH {
            enqueue(&b, &tx, raw, Some("receipt-session".into()), 42);
        }
        assert!(!b.overflow.load(Ordering::Acquire));
        enqueue(&b, &tx, raw, None, 43);
        assert!(b.overflow.load(Ordering::Acquire));
        publish_current(&Mutex::new(Some(b.clone())), &b, true, |v| assert!(!v));
        let (_, session, at) = rx.try_recv().unwrap();
        assert_eq!(session.as_deref(), Some("receipt-session"));
        assert_eq!(at, 42);
        enqueue(&b, &tx, raw, None, 44);
        assert_eq!(rx.try_iter().count(), QUEUE_LENGTH - 1);
    }

    #[test]
    fn network_observer_oversized_metadata_or_malformed_input_fails_closed() {
        for raw in [
            serde_json::to_vec(&serde_json::json!({"method":"Page.frameAttached","params":{
                "frameId":"x".repeat(network_protocol::MAX_ID + 1),"parentFrameId":"parent"}}))
            .unwrap(),
            b"not json".to_vec(),
        ] {
            let b = browser();
            let (tx, rx) = mpsc::sync_channel(QUEUE_LENGTH);
            enqueue(&b, &tx, &raw, None, 0);
            assert!(b.overflow.load(Ordering::Acquire));
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn network_observer_oversized_websocket_and_foreign_replies_do_not_disable() {
        let b = browser();
        b.replies.lock().unwrap().insert(("owned".into(), 7));
        let (tx, rx) = mpsc::sync_channel(QUEUE_LENGTH);
        let body = "x".repeat(network_protocol::MAX_MESSAGE * 4);
        // Deliberately place payload before envelope fields. Neither field order
        // nor an invalid foreign result schema may invoke terminal evidence limits.
        for raw in [
            format!(
                r#"{{"params":{{"response":{{"payloadData":"{body}"}}}},"method":"Network.webSocketFrameReceived"}}"#
            ),
            format!(r#"{{"result":{{"body":"{body}","frameTree":"not a tree"}},"id":999}}"#),
            format!(
                r#"{{"result":{{"body":"{body}","frameTree":"not a tree"}},"id":7,"sessionId":"other"}}"#
            ),
        ] {
            enqueue(&b, &tx, raw.as_bytes(), Some("reader".into()), 42);
            assert!(!b.overflow.load(Ordering::Acquire));
            assert!(rx.try_recv().is_err());
        }
        publish_current(&Mutex::new(Some(b.clone())), &b, true, |available| {
            assert!(available)
        });
        enqueue(
            &b,
            &tx,
            br#"{"sessionId":"owned","id":7,"result":{}}"#,
            None,
            43,
        );
        assert!(matches!(
            rx.try_recv().unwrap().0.event,
            Event::Ack(7, true, _)
        ));
    }

    #[test]
    fn network_observer_large_ignored_request_fields_keep_only_bounded_metadata() {
        let b = browser();
        let (tx, rx) = mpsc::sync_channel(QUEUE_LENGTH);
        for (url, remote) in [
            ("https://remote.example/", true),
            ("http://127.0.0.1/import", false),
        ] {
            let raw = serde_json::to_vec(
                &serde_json::json!({"method":"Network.requestWillBeSent","params":{
                "frameId":"frame","loaderId":"loader","requestId":"request","request":{
                    "url":url,"postData":"x".repeat(network_protocol::MAX_MESSAGE * 4),
                    "headers":{"ignored":"y".repeat(network_protocol::MAX_MESSAGE * 4)}}}}),
            )
            .unwrap();
            enqueue(&b, &tx, &raw, None, 42);
            assert!(!b.overflow.load(Ordering::Acquire));
            let message = rx.try_recv().unwrap().0;
            assert!(
                matches!(message.event, Event::Request { remote: value, .. } if value == remote)
            );
            assert!(format!("{message:?}").len() < 256);
        }
    }
}
