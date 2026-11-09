// SPDX-License-Identifier: AGPL-3.0-only
//! Experimental server mode: Rich Presence through the official Discord
//! Social SDK, without a Discord desktop client (CLAUDE.md §4).
//!
//! This is the **only** crate in the workspace that contains `unsafe` code
//! (owner sign-off 2026-11-09). It is optional (cargo feature `social-sdk` of
//! the app, off by default) and keeps the FFI surface minimal:
//!
//! - the SDK is **not** linked or bundled; the owner downloads it and points
//!   the config at it, and [`Sdk::load`] opens it at runtime;
//! - only ~50 C functions are used ([`sys`]): tokens, connect, presence and
//!   reading the signed-in account (for the UI preview);
//! - one dedicated thread owns the `Discord_Client` and is the only thread
//!   that ever calls into the SDK (including `Discord_RunCallbacks`, which
//!   runs our callbacks on that same thread);
//! - callback contexts are boxed and released through the SDK's own
//!   `userDataFree` hook, exactly like the official C++ wrapper does;
//! - every `unsafe` block carries a `// SAFETY:` comment.

use std::ffi::c_void;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use remotemusicrp_discord::{Activity, ActivityKind, StatusDisplay};
use tokio::sync::{oneshot, watch};
use zeroize::Zeroizing;

mod sys;

use sys::{Api, DString, Handle};

/// How often the SDK thread pumps `Discord_RunCallbacks` while idle.
const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// How often the signed-in account is re-read from the SDK's cache.
const USER_REFRESH: Duration = Duration::from_secs(5);
const MAX_LOG_CHARS: usize = 300;

/// The SDK library stays loaded for the life of the process (unloading a
/// library whose threads are still running is undefined behaviour).
static API: OnceLock<Api> = OnceLock::new();

fn api() -> Option<&'static Api> {
    API.get()
}

/// Connection state of the SDK client (`Discord_Client_Status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Ready,
    Reconnecting,
    Disconnecting,
    HttpWait,
    Unknown(i32),
}

impl Status {
    fn from_c(v: i32) -> Self {
        match v {
            0 => Self::Disconnected,
            1 => Self::Connecting,
            2 => Self::Connected,
            3 => Self::Ready,
            4 => Self::Reconnecting,
            5 => Self::Disconnecting,
            6 => Self::HttpWait,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Ready => "ready",
            Self::Reconnecting => "reconnecting",
            Self::Disconnecting => "disconnecting",
            Self::HttpWait => "http_wait",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub status: Status,
    /// `Discord_Client_Error` and its detail code of the last change, if any.
    pub error: Option<(i32, i32)>,
    /// The signed-in account, read once the client is ready (raw, not yet
    /// validated: callers validate before use).
    pub user: Option<User>,
}

/// The signed-in Discord account, as the SDK reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: u64,
    pub username: String,
    pub global_name: Option<String>,
    /// Avatar hash, if the account has one.
    pub avatar: Option<String>,
    pub online: OnlineStatus,
}

/// A Discord online status (`Discord_StatusType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnlineStatus {
    Online,
    Idle,
    Dnd,
    Invisible,
    Offline,
    Streaming,
    #[default]
    Unknown,
}

impl OnlineStatus {
    fn from_c(v: i32) -> Self {
        match v {
            0 => Self::Online,
            1 => Self::Offline,
            3 => Self::Idle,
            4 => Self::Dnd,
            5 => Self::Invisible,
            6 => Self::Streaming,
            _ => Self::Unknown,
        }
    }

    /// The C value, for the statuses a user may choose.
    fn settable_c(self) -> Option<i32> {
        match self {
            Self::Online => Some(0),
            Self::Idle => Some(3),
            Self::Dnd => Some(4),
            Self::Invisible => Some(5),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::Dnd => "dnd",
            Self::Invisible => "invisible",
            Self::Offline => "offline",
            Self::Streaming => "streaming",
            Self::Unknown => "unknown",
        }
    }

    /// Parse one of the statuses a user may choose.
    pub fn parse_settable(s: &str) -> Option<Self> {
        match s {
            "online" => Some(Self::Online),
            "idle" => Some(Self::Idle),
            "dnd" => Some(Self::Dnd),
            "invisible" => Some(Self::Invisible),
            _ => None,
        }
    }
}

/// Tokens from an OAuth2 exchange. Wiped from memory when dropped.
pub struct Tokens {
    pub access: Zeroizing<String>,
    pub refresh: Zeroizing<String>,
    pub expires_in_secs: u64,
    pub scopes: String,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in_secs", &self.expires_in_secs)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

pub type SdkResult<T> = Result<T, String>;

enum Cmd {
    GetToken {
        app_id: u64,
        code: Zeroizing<String>,
        verifier: Zeroizing<String>,
        redirect_uri: String,
        reply: oneshot::Sender<SdkResult<Tokens>>,
    },
    RefreshToken {
        app_id: u64,
        refresh: Zeroizing<String>,
        reply: oneshot::Sender<SdkResult<Tokens>>,
    },
    RevokeToken {
        app_id: u64,
        token: Zeroizing<String>,
        reply: oneshot::Sender<SdkResult<()>>,
    },
    /// `UpdateToken` then `Connect`.
    Connect {
        access: Zeroizing<String>,
        reply: oneshot::Sender<SdkResult<()>>,
    },
    Disconnect,
    SetActivity {
        activity: Box<Activity>,
        reply: oneshot::Sender<SdkResult<()>>,
    },
    ClearActivity,
    /// Change the account's online status (C value, already validated).
    SetOnlineStatus {
        status: i32,
        reply: oneshot::Sender<SdkResult<()>>,
    },
    /// Clear the presence, disconnect, drop the client and stop the thread.
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Handle to the SDK thread. Cheap to clone; all methods are async and never
/// block the caller.
#[derive(Debug, Clone)]
pub struct Sdk {
    tx: mpsc::Sender<Cmd>,
    state: watch::Receiver<State>,
}

/// Refuse libraries that other users could replace (code would run inside
/// our process).
fn check_library_file(path: &Path) -> SdkResult<()> {
    if !path.is_absolute() {
        return Err("the Social SDK library path must be absolute".into());
    }
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot read the Social SDK library: {:?}", e.kind()))?;
    let meta = if meta.file_type().is_symlink() {
        std::fs::metadata(path)
            .map_err(|e| format!("cannot read the Social SDK library: {:?}", e.kind()))?
    } else {
        meta
    };
    if !meta.is_file() {
        return Err("the Social SDK library path is not a file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 {
            return Err(
                "the Social SDK library is writable by other users; run chmod go-w on it".into(),
            );
        }
    }
    Ok(())
}

impl Sdk {
    /// Load the SDK from `library` and start its thread. Can only succeed
    /// once per process.
    pub fn load(library: &Path) -> SdkResult<Self> {
        if API.get().is_some() {
            return Err("the Social SDK is already loaded".into());
        }
        check_library_file(library)?;
        // SAFETY: loading a shared library runs its initialisers. The path
        // comes from the owner's config file (never from the web UI), points
        // to the Discord Social SDK they downloaded, and was checked above to
        // be a regular file not writable by other users.
        let lib = unsafe { libloading::Library::new(library) }
            .map_err(|e| format!("could not load the Social SDK library: {e}"))?;
        // SAFETY: `lib` is the Social SDK (see above) and is leaked right
        // after, so the resolved function pointers stay valid forever.
        let resolved = unsafe { Api::load(&lib) }?;
        std::mem::forget(lib);
        API.set(resolved)
            .map_err(|_| "the Social SDK is already loaded".to_owned())?;

        let (tx, rx) = mpsc::channel();
        let (state_tx, state) = watch::channel(State::default());
        std::thread::Builder::new()
            .name("discord-sdk".into())
            .spawn(move || worker(rx, state_tx))
            .map_err(|e| format!("could not start the Social SDK thread: {:?}", e.kind()))?;
        Ok(Self { tx, state })
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// Wait for the next state change.
    pub async fn changed(&mut self) -> bool {
        self.state.changed().await.is_ok()
    }

    async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<SdkResult<T>>) -> Cmd,
    ) -> SdkResult<T> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| "the Social SDK thread has stopped".to_owned())?;
        rx.await
            .map_err(|_| "the Social SDK did not answer".to_owned())?
    }

    /// Exchange an authorization code (PKCE, public client).
    pub async fn get_token(
        &self,
        app_id: u64,
        code: Zeroizing<String>,
        verifier: Zeroizing<String>,
        redirect_uri: String,
    ) -> SdkResult<Tokens> {
        self.call(|reply| Cmd::GetToken {
            app_id,
            code,
            verifier,
            redirect_uri,
            reply,
        })
        .await
    }

    pub async fn refresh_token(
        &self,
        app_id: u64,
        refresh: Zeroizing<String>,
    ) -> SdkResult<Tokens> {
        self.call(|reply| Cmd::RefreshToken {
            app_id,
            refresh,
            reply,
        })
        .await
    }

    pub async fn revoke_token(&self, app_id: u64, token: Zeroizing<String>) -> SdkResult<()> {
        self.call(|reply| Cmd::RevokeToken {
            app_id,
            token,
            reply,
        })
        .await
    }

    /// Hand the access token to the SDK and connect.
    pub async fn connect(&self, access: Zeroizing<String>) -> SdkResult<()> {
        self.call(|reply| Cmd::Connect { access, reply }).await
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    pub async fn set_activity(&self, activity: &Activity) -> SdkResult<()> {
        let activity = Box::new(activity.clone());
        self.call(|reply| Cmd::SetActivity { activity, reply })
            .await
    }

    /// Set the account's online status (online, idle, dnd or invisible).
    pub async fn set_online_status(&self, status: OnlineStatus) -> SdkResult<()> {
        let status = status
            .settable_c()
            .ok_or_else(|| "that status cannot be chosen".to_owned())?;
        self.call(|reply| Cmd::SetOnlineStatus { status, reply })
            .await
    }

    pub fn clear_activity(&self) {
        let _ = self.tx.send(Cmd::ClearActivity);
    }

    /// Clear the presence, disconnect and stop the SDK thread. Must finish
    /// before the process exits: the SDK aborts if its exit-time teardown
    /// runs while our thread is still calling into it. Safe to call twice.
    pub async fn shutdown(&self) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Shutdown { reply }).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(3), rx).await;
        }
    }
}

// ---- callback plumbing (runs on the SDK thread) ---------------------------

/// Context for one SDK callback: the reply channel, taken on first use.
struct Pending<T>(Mutex<Option<oneshot::Sender<SdkResult<T>>>>);

impl<T> Pending<T> {
    fn boxed(reply: oneshot::Sender<SdkResult<T>>) -> *mut c_void {
        Box::into_raw(Box::new(Self(Mutex::new(Some(reply))))).cast()
    }
}

/// `userDataFree` for boxes created with `Box::into_raw`.
unsafe extern "C" fn free_box<T>(p: *mut c_void) {
    if !p.is_null() {
        // SAFETY: `p` was produced by `Box::into_raw(Box<T>)` for exactly this
        // T (each registration pairs the box type with `free_box::<T>`), and
        // the SDK calls the free hook once.
        drop(unsafe { Box::from_raw(p.cast::<T>()) });
    }
}

/// Copy a callback string and release the SDK's buffer.
///
/// # Safety
/// `s` must be a string the SDK handed to a callback (owned by us).
unsafe fn take_string(api: &Api, s: DString) -> Zeroizing<String> {
    if s.ptr.is_null() {
        return Zeroizing::new(String::new());
    }
    // SAFETY: the SDK guarantees `ptr` points to `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.size) };
    let out = Zeroizing::new(String::from_utf8_lossy(bytes).into_owned());
    // SAFETY: callback strings are owned by the receiver and freed with
    // Discord_Free (as in the official C++ wrapper).
    unsafe { (api.free)(s.ptr.cast()) };
    out
}

/// Read a `ClientResult` and drop it.
///
/// # Safety
/// `r` must be the result pointer the SDK passed to a callback.
unsafe fn take_result(api: &Api, r: *mut Handle) -> SdkResult<()> {
    if r.is_null() {
        return Err("the Social SDK returned no result".into());
    }
    // SAFETY: `r` is a valid ClientResult for the duration of the callback.
    let ok = unsafe { (api.result_successful)(r) };
    let out = if ok {
        Ok(())
    } else {
        let mut s = DString {
            ptr: std::ptr::null_mut(),
            size: 0,
        };
        // SAFETY: writes an SDK-owned string into `s`, released below.
        unsafe { (api.result_error)(r, &raw mut s) };
        // SAFETY: `s` was just returned by the SDK and is ours to free.
        let msg = unsafe { take_string(api, s) };
        Err(sanitize(&msg))
    };
    // SAFETY: the callback owns the result (the C++ wrapper drops it too).
    unsafe { (api.result_drop)(r) };
    out
}

fn sanitize(s: &str) -> String {
    let s: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LOG_CHARS)
        .collect();
    if s.is_empty() {
        "the Social SDK reported an error".into()
    } else {
        s
    }
}

unsafe extern "C" fn on_token(
    result: *mut Handle,
    access: DString,
    refresh: DString,
    _token_type: i32,
    expires_in: i32,
    scopes: DString,
    user: *mut c_void,
) {
    let Some(api) = api() else { return };
    // SAFETY: SDK-owned callback strings, released here exactly once.
    let (access, refresh, scopes) = unsafe {
        (
            take_string(api, access),
            take_string(api, refresh),
            take_string(api, scopes),
        )
    };
    // SAFETY: `result` is this callback's ClientResult.
    let res = unsafe { take_result(api, result) }.map(|()| Tokens {
        access,
        refresh,
        expires_in_secs: u64::try_from(expires_in).unwrap_or(0),
        scopes: scopes.to_string(),
    });
    // SAFETY: `user` is the `Pending<Tokens>` box registered with this
    // callback; it stays alive until the SDK calls `free_box` afterwards.
    let pending = unsafe { &*user.cast::<Pending<Tokens>>() };
    if let Some(tx) = pending.0.lock().ok().and_then(|mut g| g.take()) {
        let _ = tx.send(res);
    }
}

unsafe extern "C" fn on_result(result: *mut Handle, user: *mut c_void) {
    let Some(api) = api() else { return };
    // SAFETY: `result` is this callback's ClientResult.
    let res = unsafe { take_result(api, result) };
    // SAFETY: `user` is the `Pending<()>` box registered with this callback.
    let pending = unsafe { &*user.cast::<Pending<()>>() };
    if let Some(tx) = pending.0.lock().ok().and_then(|mut g| g.take()) {
        let _ = tx.send(res);
    }
}

/// Context for `UpdateToken`: connect on success.
struct ConnectCtx {
    client: *mut Handle,
    reply: Mutex<Option<oneshot::Sender<SdkResult<()>>>>,
}

unsafe extern "C" fn on_token_set(result: *mut Handle, user: *mut c_void) {
    let Some(api) = api() else { return };
    // SAFETY: `result` is this callback's ClientResult.
    let res = unsafe { take_result(api, result) };
    // SAFETY: `user` is the `ConnectCtx` box registered with this callback.
    let ctx = unsafe { &*user.cast::<ConnectCtx>() };
    if res.is_ok() {
        // SAFETY: callbacks run on the SDK thread inside RunCallbacks, where
        // the boxed client (owned by `worker`) is alive.
        unsafe { (api.client_connect)(ctx.client) };
    }
    if let Some(tx) = ctx.reply.lock().ok().and_then(|mut g| g.take()) {
        let _ = tx.send(res);
    }
}

unsafe extern "C" fn on_status(status: i32, error: i32, detail: i32, user: *mut c_void) {
    // SAFETY: `user` is the boxed `Arc<watch::Sender<State>>` registered in
    // `worker`, alive until the client is dropped.
    let tx = unsafe { &*user.cast::<StateTx>() };
    let status = Status::from_c(status);
    tracing::info!(status = status.as_str(), error, detail, "Social SDK status");
    tx.send_modify(|s| {
        s.status = status;
        s.error = (error != 0).then_some((error, detail));
        if status == Status::Disconnected {
            s.user = None;
        }
    });
}

type StateTx = std::sync::Arc<watch::Sender<State>>;

/// Read the signed-in account from the SDK's cache (no network call).
fn current_user(api: &Api, c: *mut Handle) -> Option<User> {
    let mut u = Handle::null();
    // SAFETY: valid client on its owning thread; `u` is an out-parameter the
    // SDK initialises only when it returns true.
    if !unsafe { (api.client_get_current_user_v2)(c, &raw mut u) } {
        return None;
    }
    let u: *mut Handle = &raw mut u;
    let optional = |f: unsafe extern "C" fn(*mut Handle, *mut DString) -> bool| {
        let mut s = DString::empty();
        // SAFETY: `u` is an initialised user handle; the string is filled
        // only when the call returns true.
        let present = unsafe { f(u, &raw mut s) };
        // SAFETY: the SDK filled `s` and it is ours to free.
        present.then(|| unsafe { take_string(api, s) }.to_string())
    };
    let mut name = DString::empty();
    // SAFETY: `u` is initialised; returned strings are owned by the caller
    // and freed by `take_string` (as in the official C++ wrapper).
    let user = unsafe {
        (api.user_username)(u, &raw mut name);
        User {
            id: (api.user_id)(u),
            username: take_string(api, name).to_string(),
            global_name: optional(api.user_global_name),
            avatar: optional(api.user_avatar),
            online: OnlineStatus::from_c((api.user_status)(u)),
        }
    };
    // SAFETY: drops the handle returned by GetCurrentUserV2, exactly once.
    unsafe { (api.user_drop)(u) };
    Some(user)
}

unsafe extern "C" fn on_log(message: DString, _severity: i32, _user: *mut c_void) {
    let Some(api) = api() else { return };
    // SAFETY: log messages are owned by the receiver (C++ wrapper frees them).
    let msg = unsafe { take_string(api, message) };
    // Only errors are requested; still never log long or multi-line text.
    tracing::warn!(message = %sanitize(&msg), "Social SDK");
}

// ---- the SDK thread ---------------------------------------------------------

/// Builds a `Discord_Activity` from our activity, owns every sub-object and
/// drops them in `Drop`.
struct ActivityBuilder<'a> {
    api: &'a Api,
    activity: Handle,
}

impl<'a> ActivityBuilder<'a> {
    fn new(api: &'a Api, a: &Activity) -> Self {
        let mut activity = Handle::null();
        // SAFETY: initialises a fresh Activity in our stack slot.
        unsafe { (api.activity_init)(&raw mut activity) };
        let mut b = Self { api, activity };
        b.fill(a);
        b
    }

    fn opt(
        set: unsafe extern "C" fn(*mut Handle, *mut DString),
        target: *mut Handle,
        v: Option<&str>,
    ) {
        if let Some(v) = v {
            let mut s = DString::borrow(v);
            // SAFETY: `target` is an initialised SDK object; the SDK copies
            // the borrowed string during the call.
            unsafe { set(target, &raw mut s) };
        }
    }

    fn fill(&mut self, a: &Activity) {
        let api = self.api;
        let act = &raw mut self.activity;
        let kind = match a.kind {
            ActivityKind::Playing => 0,
            ActivityKind::Listening => 2,
            ActivityKind::Watching => 3,
            ActivityKind::Competing => 5,
        };
        // SAFETY: `act` is initialised; enum values are those of cdiscord.h.
        unsafe { (api.activity_set_type)(act, kind) };
        if let Some(name) = a.name.as_deref() {
            // SAFETY: borrowed string, copied by the SDK.
            unsafe { (api.activity_set_name)(act, DString::borrow(name)) };
        }
        Self::opt(api.activity_set_details, act, a.details.as_deref());
        Self::opt(api.activity_set_details_url, act, a.details_url.as_deref());
        Self::opt(api.activity_set_state, act, a.state.as_deref());
        Self::opt(api.activity_set_state_url, act, a.state_url.as_deref());
        if let Some(sd) = a.status_display {
            let mut v: i32 = match sd {
                StatusDisplay::Name => 0,
                StatusDisplay::State => 1,
                StatusDisplay::Details => 2,
            };
            // SAFETY: pointer to a valid enum value for the duration of the call.
            unsafe { (api.activity_set_status_display_type)(act, &raw mut v) };
        }

        if a.large_image.is_some() || a.small_image.is_some() {
            let mut assets = Handle::null();
            let p = &raw mut assets;
            // SAFETY: fresh ActivityAssets, dropped below after the SDK
            // copied it into the activity (SetAssets clones, as the C++
            // wrapper's temporary shows).
            unsafe { (api.assets_init)(p) };
            Self::opt(api.assets_set_large_image, p, a.large_image.as_deref());
            Self::opt(api.assets_set_large_text, p, a.large_text.as_deref());
            Self::opt(api.assets_set_large_url, p, a.large_url.as_deref());
            Self::opt(api.assets_set_small_image, p, a.small_image.as_deref());
            Self::opt(api.assets_set_small_text, p, a.small_text.as_deref());
            // SAFETY: both objects are initialised; then we drop our copy.
            unsafe {
                (api.activity_set_assets)(act, p);
                (api.assets_drop)(p);
            }
        }

        if a.start_unix.is_some() || a.end_unix.is_some() {
            let mut ts = Handle::null();
            let p = &raw mut ts;
            // SAFETY: fresh ActivityTimestamps, cloned by SetTimestamps, then dropped.
            unsafe {
                (api.timestamps_init)(p);
                if let Some(s) = a.start_unix {
                    (api.timestamps_set_start)(p, s);
                }
                if let Some(e) = a.end_unix {
                    (api.timestamps_set_end)(p, e);
                }
                (api.activity_set_timestamps)(act, p);
                (api.timestamps_drop)(p);
            }
        }

        for button in a.buttons.iter().take(2) {
            let mut b = Handle::null();
            let p = &raw mut b;
            // SAFETY: fresh ActivityButton with borrowed (copied) strings;
            // AddButton takes a const pointer and copies it; then dropped.
            unsafe {
                (api.button_init)(p);
                (api.button_set_label)(p, DString::borrow(&button.label));
                (api.button_set_url)(p, DString::borrow(&button.url));
                (api.activity_add_button)(act, p);
                (api.button_drop)(p);
            }
        }
    }
}

impl Drop for ActivityBuilder<'_> {
    fn drop(&mut self) {
        // SAFETY: the activity was initialised in `new` and is dropped once.
        unsafe { (self.api.activity_drop)(&raw mut self.activity) };
    }
}

fn worker(rx: mpsc::Receiver<Cmd>, state_tx: watch::Sender<State>) {
    let Some(api) = api() else { return };
    // The client lives in a Box so its address never changes.
    let mut client = Box::new(Handle::null());
    let c: *mut Handle = &raw mut *client;
    // SAFETY: initialises the one SDK client; only this thread touches it.
    unsafe { (api.client_init)(c) };
    let state_tx: StateTx = std::sync::Arc::new(state_tx);
    let status_ctx: *mut c_void = Box::into_raw(Box::new(state_tx.clone())).cast();
    // SAFETY: callbacks run on this thread inside RunCallbacks; the contexts
    // are released by the SDK through `free_box` with matching types.
    unsafe {
        (api.client_set_status_changed_callback)(c, on_status, free_box::<StateTx>, status_ctx);
        (api.client_add_log_callback)(
            c,
            on_log,
            free_box::<()>,
            std::ptr::null_mut(),
            sys::LOG_ERROR,
        );
    }

    let mut next_user_read = Instant::now();
    let reply = loop {
        match rx.recv_timeout(PUMP_INTERVAL) {
            Ok(Cmd::Shutdown { reply }) => break Some(reply),
            Ok(cmd) => handle(api, c, cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        }
        // SAFETY: called from the thread that owns the client, as required.
        unsafe { (api.run_callbacks)() };
        // While ready, re-read the account from the SDK's local cache (no
        // network call) so the preview's name, avatar and online status
        // follow changes: every 5 s, or each pump until the first read works.
        let ready = state_tx.borrow().status == Status::Ready;
        let now = Instant::now();
        if ready && now >= next_user_read {
            let user = current_user(api, c);
            let got = user.is_some();
            state_tx.send_if_modified(|s| {
                if got && s.user != user {
                    s.user = user;
                    true
                } else {
                    false
                }
            });
            next_user_read = now + if got { USER_REFRESH } else { PUMP_INTERVAL };
        }
    };
    // SAFETY: last uses of the client, on its owning thread. Pumping a few
    // times lets the presence clear reach Discord before we disconnect.
    unsafe {
        (api.client_clear_rich_presence)(c);
        for _ in 0..5 {
            (api.run_callbacks)();
            std::thread::sleep(Duration::from_millis(100));
        }
        (api.client_disconnect)(c);
        (api.run_callbacks)();
        (api.client_drop)(c);
    }
    // Dropping `rx` makes every later command fail fast instead of reaching
    // a dropped client.
    drop(rx);
    if let Some(reply) = reply {
        let _ = reply.send(());
    }
}

fn handle(api: &Api, c: *mut Handle, cmd: Cmd) {
    match cmd {
        Cmd::GetToken {
            app_id,
            code,
            verifier,
            redirect_uri,
            reply,
        } => {
            // SAFETY: valid client; borrowed strings are copied by the SDK;
            // the context box is freed by the SDK via `free_box`.
            unsafe {
                (api.client_get_token)(
                    c,
                    app_id,
                    DString::borrow(&code),
                    DString::borrow(&verifier),
                    DString::borrow(&redirect_uri),
                    on_token,
                    free_box::<Pending<Tokens>>,
                    Pending::boxed(reply),
                );
            }
        }
        Cmd::RefreshToken {
            app_id,
            refresh,
            reply,
        } => {
            // SAFETY: as for GetToken.
            unsafe {
                (api.client_refresh_token)(
                    c,
                    app_id,
                    DString::borrow(&refresh),
                    on_token,
                    free_box::<Pending<Tokens>>,
                    Pending::boxed(reply),
                );
            }
        }
        Cmd::RevokeToken {
            app_id,
            token,
            reply,
        } => {
            // SAFETY: as for GetToken.
            unsafe {
                (api.client_revoke_token)(
                    c,
                    app_id,
                    DString::borrow(&token),
                    on_result,
                    free_box::<Pending<()>>,
                    Pending::boxed(reply),
                );
            }
        }
        Cmd::Connect { access, reply } => {
            let ctx = Box::into_raw(Box::new(ConnectCtx {
                client: c,
                reply: Mutex::new(Some(reply)),
            }));
            // SAFETY: as for GetToken; `ctx` is released by the SDK through
            // `free_box::<ConnectCtx>`. `on_token_set` connects once the SDK
            // accepted the token (like the official sample).
            unsafe {
                (api.client_update_token)(
                    c,
                    sys::TOKEN_TYPE_BEARER,
                    DString::borrow(&access),
                    on_token_set,
                    free_box::<ConnectCtx>,
                    ctx.cast(),
                );
            }
        }
        Cmd::Disconnect => {
            // SAFETY: valid client on its owning thread.
            unsafe { (api.client_disconnect)(c) };
        }
        Cmd::SetActivity { activity, reply } => {
            let built = ActivityBuilder::new(api, &activity);
            let mut handle = Handle {
                opaque: built.activity.opaque,
            };
            // SAFETY: the activity is initialised and outlives the call (the
            // SDK takes what it needs before returning, like the C++
            // wrapper whose temporary Activity dies right after).
            unsafe {
                (api.client_update_rich_presence)(
                    c,
                    &raw mut handle,
                    on_result,
                    free_box::<Pending<()>>,
                    Pending::boxed(reply),
                );
            }
            drop(built);
        }
        Cmd::SetOnlineStatus { status, reply } => {
            // SAFETY: valid client on its owning thread; `status` is one of
            // the Discord_StatusType values; the context box is freed by the
            // SDK via `free_box` with the matching type.
            unsafe {
                (api.client_set_online_status)(
                    c,
                    status,
                    on_result,
                    free_box::<Pending<()>>,
                    Pending::boxed(reply),
                );
            }
        }
        Cmd::ClearActivity => {
            // SAFETY: valid client on its owning thread.
            unsafe { (api.client_clear_rich_presence)(c) };
        }
        // The worker loop handles this before `handle` is reached.
        Cmd::Shutdown { reply } => {
            let _ = reply.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_and_missing_paths() {
        assert!(check_library_file(Path::new("libdiscord_partner_sdk.so")).is_err());
        let missing = std::env::temp_dir().join("definitely-missing-sdk.so");
        assert!(check_library_file(&missing).is_err());
    }

    #[test]
    fn status_mapping() {
        assert_eq!(Status::from_c(3), Status::Ready);
        assert_eq!(Status::from_c(42), Status::Unknown(42));
        assert_eq!(Status::Ready.as_str(), "ready");
    }

    #[test]
    fn sanitize_strips_control_chars() {
        assert_eq!(sanitize("a\nb\tc"), "abc");
        assert_eq!(sanitize(""), "the Social SDK reported an error");
    }

    #[test]
    fn tokens_debug_hides_secrets() {
        let t = Tokens {
            access: Zeroizing::new("secret-access".into()),
            refresh: Zeroizing::new("secret-refresh".into()),
            expires_in_secs: 1,
            scopes: "openid".into(),
        };
        let d = format!("{t:?}");
        assert!(!d.contains("secret"));
    }
}
