//! Minimal hand-written libmpv client API surface, loaded at runtime with `libloading`.
//!
//! Why dynamic loading instead of `libmpv-sys`/`libmpv2`:
//! * no link-time dependency → the workspace builds on any machine (CI, Linux boxes without
//!   libmpv, Windows without an import library);
//! * when `libmpv-2.dll` is missing the app still starts and falls back to the stub engine
//!   (CLAUDE.md §15: "Do not block the whole app on video");
//! * we only need ~15 entry points.
//!
//! Struct layouts and enum values are from `mpv/client.h` (client API 2.x, mpv ≥ 0.35).

#![allow(non_camel_case_types, dead_code)]

use libloading::{Library, Symbol};
use std::ffi::{c_char, c_double, c_int, c_ulong, c_void, CStr, CString};
use std::path::{Path, PathBuf};

pub type mpv_handle = c_void;

// ---- enums --------------------------------------------------------------------------

pub const MPV_FORMAT_NONE: c_int = 0;
pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_OSD_STRING: c_int = 2;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_LOG_MESSAGE: c_int = 2;
pub const MPV_EVENT_GET_PROPERTY_REPLY: c_int = 3;
pub const MPV_EVENT_SET_PROPERTY_REPLY: c_int = 4;
pub const MPV_EVENT_COMMAND_REPLY: c_int = 5;
pub const MPV_EVENT_START_FILE: c_int = 6;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_CLIENT_MESSAGE: c_int = 16;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: c_int = 18;
pub const MPV_EVENT_SEEK: c_int = 20;
pub const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
pub const MPV_EVENT_QUEUE_OVERFLOW: c_int = 24;
pub const MPV_EVENT_HOOK: c_int = 25;

pub const MPV_END_FILE_REASON_EOF: c_int = 0;
pub const MPV_END_FILE_REASON_STOP: c_int = 2;
pub const MPV_END_FILE_REASON_QUIT: c_int = 3;
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;
pub const MPV_END_FILE_REASON_REDIRECT: c_int = 5;

pub const MPV_ERROR_SUCCESS: c_int = 0;

// ---- structs ------------------------------------------------------------------------

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct mpv_event_start_file {
    pub playlist_entry_id: i64,
}

#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

// ---- function table -----------------------------------------------------------------

type FnClientApiVersion = unsafe extern "C" fn() -> c_ulong;
type FnCreate = unsafe extern "C" fn() -> *mut mpv_handle;
type FnInitialize = unsafe extern "C" fn(*mut mpv_handle) -> c_int;
type FnTerminateDestroy = unsafe extern "C" fn(*mut mpv_handle);
type FnSetOptionString = unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int;
type FnSetPropertyString = unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int;
type FnGetPropertyString = unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char;
type FnGetProperty = unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnCommand = unsafe extern "C" fn(*mut mpv_handle, *const *const c_char) -> c_int;
type FnCommandAsync = unsafe extern "C" fn(*mut mpv_handle, u64, *const *const c_char) -> c_int;
type FnObserveProperty = unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut mpv_event;
type FnWakeup = unsafe extern "C" fn(*mut mpv_handle);
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnRequestLogMessages = unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> c_int;

/// Resolved entry points. Symbols borrow from `lib`, which lives as long as this struct.
pub struct LibMpv {
    _lib: Library,
    pub path: PathBuf,
    pub client_api_version: FnClientApiVersion,
    pub create: FnCreate,
    pub initialize: FnInitialize,
    pub terminate_destroy: FnTerminateDestroy,
    pub set_option_string: FnSetOptionString,
    pub set_property_string: FnSetPropertyString,
    pub get_property_string: FnGetPropertyString,
    pub get_property: FnGetProperty,
    pub free: FnFree,
    pub command: FnCommand,
    pub command_async: FnCommandAsync,
    pub observe_property: FnObserveProperty,
    pub wait_event: FnWaitEvent,
    pub wakeup: FnWakeup,
    pub error_string: FnErrorString,
    pub request_log_messages: FnRequestLogMessages,
}

unsafe impl Send for LibMpv {}
unsafe impl Sync for LibMpv {}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("libmpv not found (tried: {0})")]
    NotFound(String),
    #[error("libmpv at {0} is missing symbol {1}")]
    MissingSymbol(String, String),
    #[error("libmpv load failed: {0}")]
    Load(String),
}

/// Candidate file names per OS, in priority order.
pub fn candidate_names() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll", "libmpv.dll"]
    }
    #[cfg(target_os = "macos")]
    {
        &["libmpv.2.dylib", "libmpv.dylib"]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &["libmpv.so.2", "libmpv.so", "libmpv.so.1"]
    }
}

/// Directories searched before the system loader path.
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(p) = std::env::var("DESKTOP_IPTV_LIBMPV_DIR") {
        dirs.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            dirs.push(dir.join("lib"));
            dirs.push(dir.join("resources").join("lib"));
            // macOS .app bundle: Contents/MacOS/<exe> → Contents/Frameworks, Contents/Resources/lib
            dirs.push(dir.join("..").join("Frameworks"));
            dirs.push(dir.join("..").join("Resources").join("lib"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/lib"));
        dirs.push(PathBuf::from("/usr/local/lib"));
    }
    dirs
}

impl LibMpv {
    /// Load libmpv from an explicit path or by searching the candidate locations.
    pub fn load(explicit: Option<&Path>) -> Result<Self, LoadError> {
        let mut tried = Vec::new();
        if let Some(p) = explicit {
            tried.push(p.display().to_string());
            return Self::from_path(p);
        }
        if let Ok(p) = std::env::var("DESKTOP_IPTV_LIBMPV") {
            let p = PathBuf::from(p);
            tried.push(p.display().to_string());
            if let Ok(l) = Self::from_path(&p) {
                return Ok(l);
            }
        }
        for dir in candidate_dirs() {
            for name in candidate_names() {
                let p = dir.join(name);
                if p.exists() {
                    tried.push(p.display().to_string());
                    match Self::from_path(&p) {
                        Ok(l) => return Ok(l),
                        Err(e) => tracing::warn!(path = %p.display(), error = %e, "libmpv candidate failed"),
                    }
                }
            }
        }
        // Finally the OS loader search path (PATH / LD_LIBRARY_PATH / DYLD_*).
        for name in candidate_names() {
            tried.push(name.to_string());
            if let Ok(l) = Self::from_path(Path::new(name)) {
                return Ok(l);
            }
        }
        Err(LoadError::NotFound(tried.join(", ")))
    }

    fn from_path(path: &Path) -> Result<Self, LoadError> {
        // SAFETY: loading libmpv runs its constructors; libmpv is a well-behaved shared lib.
        let lib = unsafe { Library::new(path) }.map_err(|e| LoadError::Load(format!("{}: {e}", path.display())))?;
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                // SAFETY: the symbol type matches the C prototype in mpv/client.h.
                let s: Symbol<$ty> = unsafe { lib.get($name) }.map_err(|_| {
                    LoadError::MissingSymbol(path.display().to_string(), String::from_utf8_lossy($name).into())
                })?;
                *s
            }};
        }
        let out = Self {
            client_api_version: sym!(b"mpv_client_api_version\0", FnClientApiVersion),
            create: sym!(b"mpv_create\0", FnCreate),
            initialize: sym!(b"mpv_initialize\0", FnInitialize),
            terminate_destroy: sym!(b"mpv_terminate_destroy\0", FnTerminateDestroy),
            set_option_string: sym!(b"mpv_set_option_string\0", FnSetOptionString),
            set_property_string: sym!(b"mpv_set_property_string\0", FnSetPropertyString),
            get_property_string: sym!(b"mpv_get_property_string\0", FnGetPropertyString),
            get_property: sym!(b"mpv_get_property\0", FnGetProperty),
            free: sym!(b"mpv_free\0", FnFree),
            command: sym!(b"mpv_command\0", FnCommand),
            command_async: sym!(b"mpv_command_async\0", FnCommandAsync),
            observe_property: sym!(b"mpv_observe_property\0", FnObserveProperty),
            wait_event: sym!(b"mpv_wait_event\0", FnWaitEvent),
            wakeup: sym!(b"mpv_wakeup\0", FnWakeup),
            error_string: sym!(b"mpv_error_string\0", FnErrorString),
            request_log_messages: sym!(b"mpv_request_log_messages\0", FnRequestLogMessages),
            path: path.to_path_buf(),
            _lib: lib,
        };
        Ok(out)
    }

    pub fn api_version(&self) -> (u32, u32) {
        // SAFETY: plain call.
        let v = unsafe { (self.client_api_version)() };
        (((v >> 16) & 0xffff) as u32, (v & 0xffff) as u32)
    }

    pub fn err_str(&self, code: c_int) -> String {
        // SAFETY: mpv_error_string returns a static string.
        unsafe {
            let p = (self.error_string)(code);
            if p.is_null() {
                format!("mpv error {code}")
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }
}

pub fn cstr(s: &str) -> CString {
    // Interior NULs cannot come from a URL/option name; replace defensively.
    CString::new(s.replace('\0', "")).expect("no interior NUL")
}
