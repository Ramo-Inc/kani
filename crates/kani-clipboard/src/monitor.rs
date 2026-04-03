//! Clipboard change detection.
//!
//! macOS: Poll NSPasteboard.changeCount every 250ms via pbpaste.
//! Windows: AddClipboardFormatListener (post-MVP, requires HWND).

use kani_proto::clipboard::ClipboardMessage;
#[cfg(target_os = "macos")]
use std::time::Duration;
use tokio::sync::mpsc;

/// Trait for clipboard monitoring.
pub trait ClipboardMonitor: Send + Sync {
    /// Start monitoring. Sends ClipboardMessage when clipboard changes.
    fn start(&self) -> mpsc::Receiver<ClipboardMessage>;
    /// Stop monitoring.
    fn stop(&self);
}

// macOS implementation
#[cfg(target_os = "macos")]
pub mod macos {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    pub struct MacOSClipboardMonitor {
        running: Arc<AtomicBool>,
        poll_interval: Duration,
    }

    impl MacOSClipboardMonitor {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
                poll_interval: Duration::from_millis(250),
            }
        }

        pub fn with_interval(interval: Duration) -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
                poll_interval: interval,
            }
        }
    }

    impl Default for MacOSClipboardMonitor {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ClipboardMonitor for MacOSClipboardMonitor {
        fn start(&self) -> mpsc::Receiver<ClipboardMessage> {
            let (tx, rx) = mpsc::channel(16);
            let running = self.running.clone();
            let interval = self.poll_interval;
            running.store(true, Ordering::Relaxed);

            tokio::spawn(async move {
                let mut last_content = String::new();
                while running.load(Ordering::Relaxed) {
                    // Use pbpaste to read clipboard (simpler than ObjC FFI for MVP)
                    if let Ok(output) = tokio::process::Command::new("pbpaste").output().await {
                        if output.status.success() {
                            let content = String::from_utf8_lossy(&output.stdout).to_string();
                            if content != last_content && !content.is_empty() {
                                last_content = content.clone();
                                let _ = tx.send(ClipboardMessage::Text(content)).await;
                            }
                        }
                    }
                    tokio::time::sleep(interval).await;
                }
            });

            rx
        }

        fn stop(&self) {
            self.running
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Write text to the local clipboard.
#[cfg(target_os = "macos")]
pub async fn write_to_clipboard(text: &str) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }
    child.wait().await?;
    Ok(())
}

/// Write text to the local clipboard via Win32 API.
#[cfg(target_os = "windows")]
pub async fn write_to_clipboard(text: &str) -> Result<(), std::io::Error> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || windows::write_clipboard_text(&text))
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(target_os = "windows")]
pub mod windows {
    use super::*;
    use ::windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
    use ::windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
        OpenClipboard, RemoveClipboardFormatListener, SetClipboardData,
    };
    use ::windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
        RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HWND_MESSAGE, MSG,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_DESTROY, WNDCLASSW,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const CF_UNICODETEXT: u32 = 13;

    thread_local! {
        static CLIPBOARD_TX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    pub struct WindowsClipboardMonitor {
        running: Arc<AtomicBool>,
    }

    impl WindowsClipboardMonitor {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl Default for WindowsClipboardMonitor {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ClipboardMonitor for WindowsClipboardMonitor {
        fn start(&self) -> mpsc::Receiver<ClipboardMessage> {
            let (tx, rx) = mpsc::channel(16);
            let running = self.running.clone();
            running.store(true, Ordering::Relaxed);

            std::thread::spawn(move || {
                run_clipboard_listener(tx, running);
            });

            rx
        }

        fn stop(&self) {
            self.running.store(false, Ordering::Relaxed);
        }
    }

    fn run_clipboard_listener(tx: mpsc::Sender<ClipboardMessage>, running: Arc<AtomicBool>) {
        unsafe {
            tracing::info!("Clipboard listener thread starting");
            let class_name = wide_string("KaniClipboardClass");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(clipboard_wnd_proc),
                hInstance: ::windows::Win32::Foundation::HINSTANCE::default(),
                lpszClassName: ::windows::core::PCWSTR(class_name.as_ptr()),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                ::windows::core::PCWSTR(class_name.as_ptr()),
                ::windows::core::PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(wc.hInstance),
                None,
            );

            let hwnd = match hwnd {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!(error = %e, "Clipboard: CreateWindowExW failed");
                    return;
                }
            };

            if let Err(e) = AddClipboardFormatListener(hwnd) {
                tracing::error!(error = %e, "Clipboard: AddClipboardFormatListener failed");
                return;
            }
            tracing::info!("Clipboard listener registered (AddClipboardFormatListener)");

            CLIPBOARD_TX.with(|cell| {
                cell.set(Box::into_raw(Box::new(tx)) as usize);
            });

            let mut msg = MSG::default();
            while running.load(Ordering::Relaxed) {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = RemoveClipboardFormatListener(hwnd);

            CLIPBOARD_TX.with(|cell| {
                let ptr = cell.get();
                if ptr != 0 {
                    let _ = Box::from_raw(ptr as *mut mpsc::Sender<ClipboardMessage>);
                    cell.set(0);
                }
            });
        }
    }

    unsafe extern "system" fn clipboard_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLIPBOARDUPDATE => {
                match read_clipboard_text() {
                    Some(text) if !text.is_empty() => {
                        let text_preview: String = text.chars().take(50).collect();
                        tracing::info!(
                            len = text.len(),
                            preview = %text_preview,
                            "WM_CLIPBOARDUPDATE: clipboard text detected"
                        );
                        CLIPBOARD_TX.with(|cell| {
                            let ptr = cell.get();
                            if ptr != 0 {
                                let tx =
                                    unsafe { &*(ptr as *const mpsc::Sender<ClipboardMessage>) };
                                match tx.try_send(ClipboardMessage::Text(text)) {
                                    Ok(()) => tracing::info!("Clipboard change sent to channel"),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Clipboard channel send failed")
                                    }
                                }
                            } else {
                                tracing::warn!("WM_CLIPBOARDUPDATE: CLIPBOARD_TX is null");
                            }
                        });
                    }
                    Some(_) => {
                        tracing::debug!("WM_CLIPBOARDUPDATE: empty text, skipping");
                    }
                    None => {
                        tracing::debug!(
                            "WM_CLIPBOARDUPDATE: no text data (non-text clipboard content?)"
                        );
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Read UTF-16 text from the clipboard via Win32 API.
    fn read_clipboard_text() -> Option<String> {
        unsafe {
            OpenClipboard(None).ok()?;
            let result = (|| -> Option<String> {
                let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
                let hglobal = HGLOBAL(handle.0);
                let ptr = GlobalLock(hglobal) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let mut len = 0;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(hglobal);
                Some(text)
            })();
            let _ = CloseClipboard();
            result
        }
    }

    /// Write UTF-16 text to the clipboard via Win32 API.
    pub fn write_clipboard_text(text: &str) -> Result<(), std::io::Error> {
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let byte_size = wide.len() * std::mem::size_of::<u16>();

            let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_size)
                .map_err(|e| std::io::Error::other(format!("GlobalAlloc: {e}")))?;

            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                return Err(std::io::Error::other("GlobalLock returned null"));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hmem);

            OpenClipboard(None)
                .map_err(|e| std::io::Error::other(format!("OpenClipboard: {e}")))?;
            let _ = EmptyClipboard();
            let result = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0)))
                .map_err(|e| std::io::Error::other(format!("SetClipboardData: {e}")));
            let _ = CloseClipboard();
            result?;
            // Clipboard owns the memory now — do not GlobalFree.
            Ok(())
        }
    }

    fn wide_string(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
