//! Windows: keep mpv's child HWND *below* the WebView2 child so the transparent webview
//! composites over the video. mpv creates its child (`WS_CHILD | WS_VISIBLE`, class "mpv") on its
//! own GUI thread shortly after `mpv_initialize`, so we poll for it briefly.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, SetWindowPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Returns the mpv child HWND once it exists.
pub fn find_mpv_child(parent: HWND) -> Option<HWND> {
    let class = wide("mpv");
    // SAFETY: plain Win32 call with a valid parent and NUL-terminated class name.
    let h = unsafe { FindWindowExW(parent, std::ptr::null_mut(), class.as_ptr(), std::ptr::null()) };
    if h.is_null() {
        None
    } else {
        Some(h)
    }
}

/// Spawn a short poller that pushes the mpv child to the bottom of the sibling z-order.
pub fn push_mpv_child_to_bottom(parent_wid: i64) {
    std::thread::Builder::new()
        .name("mpv-zorder".into())
        .spawn(move || {
            // HWND is a raw pointer type; convert inside the thread so the closure stays `Send`.
            let parent = parent_wid as isize as HWND;
            for _ in 0..100 {
                if let Some(child) = find_mpv_child(parent) {
                    // SAFETY: both handles are valid windows in this process.
                    let ok = unsafe {
                        SetWindowPos(child, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
                    };
                    tracing::info!(ok = ok != 0, "mpv child window pushed to bottom of z-order");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            tracing::warn!("mpv child window not found within 3 s; z-order left as created");
        })
        .ok();
}
