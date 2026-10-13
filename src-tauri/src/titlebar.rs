//! macOS: keep the native title bar from swallowing the page's own.
//!
//! With `TitleBarStyle::Overlay` AppKit still keeps its title bar container
//! view on top of the web view, across the whole width of the window, and that
//! view takes the mouse: nothing the page draws there gets a click or a hover.
//! Centring the traffic lights in a 40 pt bar makes it 41 pt tall, so it
//! covered the page's entire bar (sidebar, back, forward). Narrowing it to the
//! traffic lights leaves them working and hands the rest to the page, which
//! drags the window itself (data-tauri-drag-region).
//!
//! AppKit lays the container out again on resize and on entering or leaving
//! full screen, so this runs after each of those too.

use objc2_app_kit::{NSAutoresizingMaskOptions, NSWindow, NSWindowButton};
use tauri::{Runtime, WebviewWindow};

/// Room to the right of the zoom button, so its hover ring is not clipped.
const MARGIN: f64 = 12.0;

pub fn narrow_to_traffic_lights<R: Runtime>(window: &WebviewWindow<R>) {
    let Ok(ptr) = window.ns_window() else { return };
    let ptr = ptr as usize;
    // AppKit views are main-thread only; window events can arrive elsewhere.
    let _ = window.run_on_main_thread(move || unsafe {
        let ns_window = &*(ptr as *const NSWindow);
        let (Some(close), Some(zoom)) = (
            ns_window.standardWindowButton(NSWindowButton::CloseButton),
            ns_window.standardWindowButton(NSWindowButton::ZoomButton),
        ) else {
            return;
        };
        let Some(container) = close.superview().and_then(|view| view.superview()) else {
            return;
        };
        let zoom_frame = zoom.frame();
        let mut frame = container.frame();
        frame.size.width = zoom_frame.origin.x + zoom_frame.size.width + MARGIN;
        // Keep its top edge pinned, and its width from growing back with the window.
        container.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        container.setFrame(frame);
    });
}
