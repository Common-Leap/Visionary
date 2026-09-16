//! Jorge rust_extender TCP simple_server — :7878, `<TCP_MESSAGE>` framing.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;

use parking_lot::Mutex;

static LISTEN_FD: AtomicI32 = AtomicI32::new(-1);
static CLIENT_FD: AtomicI32 = AtomicI32::new(-1);
// Atomic (not Mutex): read by the game thread every frame, written on the server thread —
// a contended parking_lot lock can park a waiter forever in this environment.
static CLIENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static OUTBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INBOUND: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RECV_BUF: Mutex<String> = Mutex::new(String::new());
// `handshake` runs on the accept thread while `drain` runs on the sender thread.  Both write
// to the same stream, so a reconnect with queued frames could otherwise interleave bytes from
// the handshake and the first queued message and make the XML-like framing unrecoverable.
static SEND_LOCK: Mutex<()> = Mutex::new(());
static mut SOCKET_POOL: [u8; 0x40000] = [0; 0x40000];

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 0xffff;
const SO_REUSEADDR: i32 = 4;
const OUTBOX_CAP: usize = 8192;
const SEND_BATCH_CAP: usize = 256;
const FRAME_OPEN: &str = "<TCP_MESSAGE>";
const FRAME_CLOSE: &str = "</TCP_MESSAGE>";
const PONG_INNER: &str = r#"{"header":"Pong","body":"{}"}"#;

extern "C" {
    #[link_name = "\u{1}_ZN2nn6socket4RecvEiPvmi"]
    fn Recv(socket: i32, buffer: *mut u8, bufferLength: u64, flags: i32) -> i64;
    #[link_name = "\u{1}_ZN2nn6socket8ShutdownEii"]
    fn Shutdown(socket: i32, how: i32) -> i32;
    #[link_name = "\u{1}_ZN2nn6socket5CloseEi"]
    fn Close(socket: i32) -> i32;
}

pub fn start(port: u16) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::slight::diag::note(format!("SRV start(:{port}) — spawning server_loop"));
    thread::spawn(move || server_loop(port));
    // Dedicated sender: the ONLY place blocking Send() runs. Keeps it off the game thread so a
    // slow RPM can never stall a frame (it just backs up in OUTBOX). See `queue`.
    thread::spawn(sender_loop);
}

/// Sleep via nn::os directly — std::thread::sleep never wakes under skyline on Ryujinx
/// (observed: both server threads went permanently silent after their first sleep; the
/// skyline panic handler also uses nn::os::SleepThread, not std).
fn sleep_ms(ms: u64) {
    unsafe {
        nnsdk::nn::os::SleepThread(nnsdk::nn::TimeSpan {
            nanoseconds: ms * 1_000_000,
        });
    }
}

fn sender_loop() {
    crate::slight::diag::note("SND thread entered");
    loop {
        let fd = CLIENT_FD.load(Ordering::Acquire);
        if fd >= 0 {
            unsafe { drain(fd) };
        }
        sleep_ms(4);
    }
}

/// Messages queued and not yet sent (diagnostic — growth = sender thread dead/stuck).
pub fn outbox_depth() -> usize {
    OUTBOX.try_lock().map(|out| out.len()).unwrap_or_default()
}

fn server_loop(port: u16) {
    unsafe {
        crate::slight::diag::note("SRV thread entered — waiting for first game frame");
        // Touching nn::socket BEFORE it is initialized crashes/hangs this thread, and whether
        // skyline's logger has initialized it yet is a boot-order race (lost on some boots).
        // The per-frame driver ticking means boot init — including skyline's socket init — is
        // done, so gate on that before the first socket call.
        while !crate::slight::agent_extender::driver_has_ticked() {
            sleep_ms(250);
        }
        crate::slight::diag::note("SRV game frames running — probing sockets");
        // Do NOT call nn::socket::Initialize unconditionally: skyline's own TCP logger (:6969)
        // already initializes nn::socket in this process, and the nn SDK ABORTS on double-init
        // (on Ryujinx this silently killed the server thread before the socket was ever
        // created — :7878 never bound). Probe with Socket() first; only Initialize if sockets
        // aren't up yet (e.g. environments where the skyline logger is disabled).
        let mut listen = nnsdk::nn::socket::Socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        crate::slight::diag::note(format!("SRV socket probe -> {listen}"));
        if listen < 0 {
            let rc = nnsdk::nn::socket::Initialize(
                std::ptr::addr_of_mut!(SOCKET_POOL).cast::<u8>(),
                0x40000,
                0x20000,
                0x20,
            );
            crate::slight::diag::note(format!("SRV nn::socket::Initialize rc={rc}"));
            if rc != 0 {
                skyline::println!("[SLight] nn::socket::Initialize failed: {rc}");
                return;
            }
            listen = nnsdk::nn::socket::Socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            crate::slight::diag::note(format!("SRV socket retry -> {listen}"));
        }
        if listen < 0 {
            skyline::println!("[SLight] socket create failed ({listen}) — no debuggable server");
            return;
        }
        LISTEN_FD.store(listen, Ordering::Release);

        let reuse: i32 = 1;
        nnsdk::nn::socket::SetSockOpt(
            listen,
            SOL_SOCKET,
            SO_REUSEADDR,
            &reuse as *const i32 as *const u8,
            4,
        );

        let mut addr: nnsdk::nn::socket::SockAddrIn = std::mem::zeroed();
        addr.sin_len = 16;
        addr.sin_family = AF_INET as u8;
        addr.sin_port = nnsdk::nn::socket::InetHtons(port);
        addr.sin_addr = [0, 0, 0, 0];

        let bind_rc = nnsdk::nn::socket::Bind(
            listen,
            &addr as *const _ as *const nnsdk::root::sockaddr,
            16,
        );
        crate::slight::diag::note(format!("SRV bind :{port} rc={bind_rc}"));
        if bind_rc != 0 {
            report_bind_failure(port, bind_rc);
            return;
        }

        nnsdk::nn::socket::Listen(listen, 2);
        crate::slight::diag::note(format!("SRV listening on :{port}"));
        skyline::println!("[SLight] Accepting clients from debbugable server on :{port}");

        loop {
            let mut len: u32 = 16;
            let mut peer: nnsdk::nn::socket::SockAddrIn = std::mem::zeroed();
            let client = nnsdk::nn::socket::Accept(
                listen,
                &mut peer as *mut _ as *mut nnsdk::root::sockaddr,
                &mut len,
            ) as i32;
            if client < 0 {
                continue;
            }
            RECV_BUF.lock().clear();
            // Keep the client unpublished while the two handshake frames are sent.  The sender
            // thread keys off CLIENT_FD, so publishing it first would let queued application
            // frames race ahead of RemoveAll/GiveClientId on a reconnect.
            let Some(cid) = handshake(client) else {
                crate::slight::diag::note(format!(
                    "SRV handshake failed fd={client} — waiting for the next client"
                ));
                close_client(client);
                continue;
            };
            CLIENT_FD.store(client, Ordering::Release);
            crate::rust_extender::debuggable_server::on_rpm_client_connected(cid);
            // Inbound only. Recv blocks until data/disconnect; outbound is the sender thread's job.
            while CLIENT_FD.load(Ordering::Acquire) == client {
                recv_once(client);
            }
            close_client(client);
        }
    }
}

/// The one line a reader should be able to grep `diag.txt` for. Kept short and unmistakable so
/// it survives being pasted out of a chat log; the deploy script prints the same phrase.
pub const DOUBLE_PLUGIN_BANNER: &str = "!!!! SLIGHT: PORT ALREADY BOUND — A SECOND COPY OF THIS \
                                        PLUGIN IS PROBABLY LOADED";

/// Say out loud what a failed bind almost always means, because the silent version of this cost
/// about six rounds of misdiagnosis.
///
/// Skyline loads **every file** in `romfs:/skyline/plugins/` as a plugin, extension ignored, so a
/// `lib_effect_viewer.nro.bak` left beside the real one runs a second full copy of us: two sets of
/// ACMD hooks, two per-frame drivers, and two servers racing for this port. The visible symptom is
/// a hard 60→30 fps drop on entering training mode, which looks like a performance regression in
/// whatever was last changed and is not one.
///
/// The bind failure was already logged — as `SRV bind :7878 rc=-1`, one lowercase line among
/// thousands, which nobody reads as "you have two plugins installed". What makes this loud is not
/// the flush; it is naming the cause, the remedy, and a second check the reader can run.
///
/// **The second check is the honest part.** A bind can fail for other reasons (a port set in
/// `gateway.txt` that something else owns), so the banner does not assert the diagnosis, it points
/// at the corroborating evidence: each instance runs its own server thread and its own diag flush,
/// so `SRV thread entered` appears **once per loaded copy**. Two of them is the proof.
///
/// It also explains the tell that misled us the first time. [`diag::start_session`] truncates and
/// rewrites the header, and both copies do it during plugin init, so whichever loads *second* wins
/// the `build=` line — a reader who just deployed build X sees build Y at the top of the file and
/// concludes their deploy did not take.
///
/// [`diag::start_session`]: crate::slight::diag::start_session
fn report_bind_failure(port: u16, rc: u32) {
    use crate::slight::diag;
    let build = crate::slight::effect_viewer::live_eff::BUILD_TAG;
    diag::note(DOUBLE_PLUGIN_BANNER);
    diag::note(format!(
        "     bind :{port} failed rc={rc}; this copy is build={build}"
    ));
    diag::note("     check: `SRV thread entered` appears once per loaded copy — two = two plugins");
    diag::note("     note:  the build= header is written by whichever copy loads LAST, not yours");
    diag::note("     fix:   romfs:/skyline/plugins/ must hold ONE lib_effect_viewer.nro and no");
    diag::note("            copies of it under any other name — Skyline loads every file there,");
    diag::note("            .bak and .old included. Re-run the deploy script; it now refuses.");
    diag::note("     without this server there is no live preview; the game is otherwise fine.");
    // Flush here rather than leaving it to the per-frame driver. This runs once, on the server
    // thread, and the buffer it would otherwise sit in is capped at 40000 lines and shared with
    // two chatty instances — the one message that must not be dropped should not queue behind
    // 30 frames of SPAWN lines from a plugin the reader does not know is running.
    diag::flush();
    skyline::println!("[SLight] bind :{port} failed rc={rc} — second plugin copy? build={build}");
}

unsafe fn handshake(client: i32) -> Option<u64> {
    let cid = CLIENT_ID.fetch_add(1, Ordering::SeqCst) + 1;
    // Keep both handshake frames together with respect to the dedicated sender.  This matters
    // after a reconnect, when a failed send may have left frames waiting in OUTBOX.
    let _send_guard = send_guard();
    if send_frame_unlocked(client, r#"{"header":"RemoveAll","body":"{}"}"#) < 0 {
        return None;
    }
    if send_frame_unlocked(
        client,
        &format!(
            r#"{{"header":"GiveClientId","body":"{{\"GiveClientId\":{{\"client_id\":{cid}}}}}"}}"#
        ),
    ) < 0
    {
        return None;
    }
    Some(cid)
}

/// Acquire the stream-write lock without parking a thread in the Skyline environment.
fn send_guard() -> parking_lot::MutexGuard<'static, ()> {
    loop {
        if let Some(guard) = SEND_LOCK.try_lock() {
            return guard;
        }
        sleep_ms(1);
    }
}

/// Write one complete framed message.  TCP `Send` is allowed to return a short write; treating
/// the first return value as the whole frame corrupts the next frame when a large donor payload
/// or a congested emulator splits it.
unsafe fn send_frame_unlocked(client: i32, inner: &str) -> i64 {
    let msg = format!("{FRAME_OPEN}{inner}{FRAME_CLOSE}");
    let bytes = msg.as_bytes();
    let mut sent = 0usize;
    while sent < bytes.len() {
        let remaining = bytes.len() - sent;
        let n =
            nnsdk::nn::socket::Send(client, bytes.as_ptr().add(sent), remaining as u64, 0) as i64;
        if n <= 0 || n as usize > remaining {
            return -1;
        }
        sent += n as usize;
    }
    sent as i64
}

unsafe fn recv_once(client: i32) {
    let mut chunk = [0u8; 4096];
    let n = Recv(client, chunk.as_mut_ptr(), chunk.len() as u64, 0);
    if n <= 0 {
        // A reset/error is just as terminal as EOF.  Leaving the fd live strands the accept
        // loop in this dead client forever, so the editor's reconnect attempts all fail.
        mark_client_disconnected(client);
        crate::slight::diag::note(format!("SRV recv fd={client} rc={n} — client disconnected"));
        return;
    }
    let text = String::from_utf8_lossy(&chunk[..n as usize]);
    let mut buf = RECV_BUF.lock();
    buf.push_str(&text);
    extract_inbound(&mut buf);
}

fn extract_inbound(buf: &mut String) {
    for payload in extract_payloads(buf) {
        inbound_push(payload);
    }
}

/// Extract framed or legacy newline-delimited payloads without touching plugin state.  Keeping
/// this pure makes the stream recovery rules testable on the host and avoids hiding parser bugs
/// behind the game-only inbound queue.
fn extract_payloads(buf: &mut String) -> Vec<String> {
    let mut payloads = Vec::new();
    loop {
        let start = buf.find(FRAME_OPEN);
        let end = buf.find(FRAME_CLOSE);
        if let (Some(s), Some(e)) = (start, end) {
            if e >= s {
                let payload = buf[s + FRAME_OPEN.len()..e].trim().to_string();
                *buf = buf[e + FRAME_CLOSE.len()..].to_string();
                if !payload.is_empty() {
                    payloads.push(payload);
                }
                continue;
            }
            // A stale close tag before the next opening tag would otherwise remain forever and
            // poison every subsequent frame on this connection.
            *buf = buf[e + FRAME_CLOSE.len()..].to_string();
            continue;
        }

        if start.is_none() {
            // Drop an orphan closing tag even when there is no opening tag yet. This is the same
            // recovery policy as the desktop parser and handles a torn previous frame.
            if let Some(e) = end {
                *buf = buf[e + FRAME_CLOSE.len()..].to_string();
                continue;
            }
        }

        if let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            *buf = buf[nl + 1..].to_string();
            if !line.is_empty() {
                payloads.push(line);
            }
            continue;
        }

        // Runaway guard: only discard when we're NOT mid-message. A large frame in flight
        // (e.g. a ~1.3 MB base64 donor eff) legitimately exceeds any small cap before its
        // closing tag arrives — clearing it here silently dropped donor_bytes. Keep
        // accumulating while an opening tag is present; only clear genuine garbage.
        if buf.len() > 64 * 1024 * 1024 && !buf.contains("<TCP_MESSAGE>") {
            buf.clear();
        }
        break;
    }
    payloads
}

/// Server thread → INBOUND. try_lock + SleepThread retry — never parks (parked waiters
/// never wake in this environment).
fn inbound_push(payload: String) {
    // Heartbeats belong to the transport rather than the game-frame pump. Answer here so a
    // paused or heavily loaded game still proves that its socket thread is alive.
    if is_ping(&payload) {
        queue_pong();
        return;
    }
    // Timing rules are pure shared state. Install them on receipt so the next ACMD coroutine
    // boundary can see a frame-0 edit; the general parser below remains responsible for all
    // commands and game-thread-only updates.
    if crate::rust_extender::debuggable_server::apply_timing_rules_from_network(&payload) {
        return;
    }
    loop {
        if let Some(mut q) = INBOUND.try_lock() {
            q.push(payload);
            return;
        }
        sleep_ms(1);
    }
}

fn is_ping(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .is_some_and(|value| {
            value.get("command").and_then(|command| command.as_str()) == Some("ping")
        })
}

pub fn queue(inner: String) {
    // Push only — the sender thread does the (blocking) Send. Never send from here: this is
    // called on the game thread, and a blocking Send while RPM is slow would stall the frame.
    // try_lock + short spin — NEVER park the game thread (parked waiters never wake here).
    // Worst case the message is dropped; RPM gets a full re-sync on reconnect anyway.
    for _ in 0..1000 {
        if let Some(mut out) = OUTBOX.try_lock() {
            // Bound growth if RPM is stuck; drop newest.
            if out.len() < OUTBOX_CAP {
                out.push(inner);
            }
            return;
        }
        core::hint::spin_loop();
    }
}

unsafe fn drain(client: i32) {
    // Take the batch out under the lock, then release before the blocking sends so `queue`
    // (game thread) never waits on the socket via the OUTBOX lock. Sender thread may sleep.
    let msgs: Vec<String> = loop {
        if let Some(mut out) = OUTBOX.try_lock() {
            let count = out.len().min(SEND_BATCH_CAP);
            break out.drain(..count).collect();
        }
        sleep_ms(1);
    };
    if !msgs.is_empty() {
        crate::slight::diag::note(format!("SND drain {} msgs", msgs.len()));
    }
    let _send_guard = send_guard();
    for (index, msg) in msgs.iter().enumerate() {
        let rc = send_frame_unlocked(client, msg);
        if rc < 0 {
            crate::slight::diag::note(format!("SND send failed rc={rc}"));
            // The current frame may have been partially written, so it must be retried only on
            // a fresh TCP connection. Requeue it and every later frame in their original order.
            requeue_front(msgs.into_iter().skip(index).collect());
            mark_client_disconnected(client);
            return;
        }
    }
}

fn requeue_front(unsent: Vec<String>) {
    if unsent.is_empty() {
        return;
    }
    loop {
        if let Some(mut out) = OUTBOX.try_lock() {
            let mut combined = unsent;
            combined.extend(out.drain(..));
            // Keep the same oldest-first/drop-newest policy as `queue` while preserving all
            // frames that were already in flight ahead of newer edits.
            combined.truncate(OUTBOX_CAP);
            *out = combined;
            return;
        }
        sleep_ms(1);
    }
}

fn mark_client_disconnected(client: i32) {
    if CLIENT_FD
        .compare_exchange(client, -1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // Wake the accept thread if it is blocked in Recv while the sender discovered the
        // failure. The accept thread closes the descriptor after its receive loop exits.
        unsafe {
            let _ = Shutdown(client, 2);
        }
    }
}

fn close_client(client: i32) {
    // Do not recycle the descriptor while the sender still has a write in progress.
    let _send_guard = send_guard();
    unsafe {
        let _ = Close(client);
    }
}

/// Queue the heartbeat response expected by the desktop `GameLink` ping probe.
fn queue_pong() {
    loop {
        if let Some(mut out) = OUTBOX.try_lock() {
            if !out.iter().any(|message| message == PONG_INNER) {
                out.insert(0, PONG_INNER.to_owned());
                out.truncate(OUTBOX_CAP);
            }
            return;
        }
        sleep_ms(1);
    }
}

pub fn has_client() -> bool {
    CLIENT_FD.load(Ordering::Acquire) >= 0
}

pub fn client_id() -> u64 {
    CLIENT_ID.load(Ordering::Acquire)
}

/// Game thread. try_lock — under contention with the server thread's push, skip a frame
/// instead of parking (parked waiters never wake here).
pub fn take_inbound() -> Vec<String> {
    match INBOUND.try_lock() {
        Some(mut q) => q.drain(..).collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pong_is_a_desktop_compatible_framed_envelope() {
        let frame = format!("{FRAME_OPEN}{PONG_INNER}{FRAME_CLOSE}");
        let payload = &frame[FRAME_OPEN.len()..frame.len() - FRAME_CLOSE.len()];
        let value: serde_json::Value = serde_json::from_str(payload).expect("valid Pong JSON");
        assert_eq!(value["header"], "Pong");
        assert_eq!(value["body"], "{}");
    }

    #[test]
    fn extractor_reassembles_torn_frames_and_recovers_from_orphan_close() {
        let mut buf = format!("stale{FRAME_CLOSE}{FRAME_OPEN}{{\"command\":\"ping\"}}");
        assert!(extract_payloads(&mut buf).is_empty());
        buf.push_str(FRAME_CLOSE);
        assert_eq!(
            extract_payloads(&mut buf),
            vec![r#"{"command":"ping"}"#.to_owned()]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn extractor_handles_multiple_frames_in_one_read() {
        let mut buf = format!("{FRAME_OPEN}one{FRAME_CLOSE}{FRAME_OPEN}two{FRAME_CLOSE}");
        assert_eq!(extract_payloads(&mut buf), vec!["one", "two"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn ping_detection_is_exact() {
        assert!(is_ping(r#"{"command":"ping","echo":4}"#));
        assert!(!is_ping(r#"{"command":"live_eff_ping"}"#));
        assert!(!is_ping("not json"));
    }
}
