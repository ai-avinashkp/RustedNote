//! Single-instance enforcement (Windows): a second launch — e.g. double-
//! clicking another file in Explorer while RustedNote is already open — hands
//! its file over to the running instance instead of opening a second window.
//!
//! Without this, "Open with" started a brand-new process per file, with no
//! link between the windows: any Untitled-tab content typed into the first
//! window was one accidental close of *that* window away from being lost.
//! See PACKAGING.md.
//!
//! Mechanism: a named pipe, created with `FILE_FLAG_FIRST_PIPE_INSTANCE` so
//! creation itself doubles as the single-instance check — the OS guarantees
//! only one process can ever hold that flag on a given pipe name, so there's
//! no separate mutex and no race between two instances launched together.
//! The process that wins keeps a background thread accepting connections for
//! the rest of its life; every later launch just connects once, writes its
//! file path (UTF-8, or nothing if it has none), and exits.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// One later launch reaching the primary instance: `Some(path)` when it named
/// a file to open (as a new tab), `None` when it named none (e.g. a plain
/// shortcut with no file argument) — either way the window should come to
/// the front, so a relaunch is never silently a no-op.
pub type Handoff = Option<PathBuf>;

/// Attempt to become the primary instance.
///
/// `Some(rx)` means this process IS primary: open the window as normal, and
/// poll `rx` for handoffs from later launches — see `main.rs`.
///
/// On Windows, losing the race hands the file (if any) to the primary
/// instance and calls `std::process::exit` directly, so this function does
/// not return at all in that case; `None` is unused there but kept so
/// callers don't have to special-case platforms. On other platforms this
/// always returns `Some` of a receiver that never yields anything — no IPC is
/// attempted there yet, so every launch opens its own window, same as before
/// this fix.
pub fn claim_or_forward(path: Option<&std::path::Path>) -> Option<Receiver<Handoff>> {
    imp::claim_or_forward(path)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsStr;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use std::sync::mpsc;
    use std::thread;

    use winapi::shared::minwindef::DWORD;
    use winapi::shared::winerror::{ERROR_ACCESS_DENIED, ERROR_PIPE_CONNECTED};
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::ReadFile;
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::namedpipeapi::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe};
    use winapi::um::winbase::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use winapi::um::winnt::HANDLE;

    const PIPE_NAME: &str = r"\\.\pipe\RustedNote.SingleInstance";
    /// Comfortably past any real filesystem path; a message longer than this
    /// would mean something is wrong upstream, not a path to honor.
    const MAX_MESSAGE: usize = 64 * 1024;

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(once(0)).collect()
    }

    /// Create one pipe instance. `first` gates it with
    /// `FILE_FLAG_FIRST_PIPE_INSTANCE` — the atomic single-instance check —
    /// for the very first instance only; later instances (opened to accept
    /// the *next* client once the primary is already established) omit it,
    /// since by then being first no longer matters.
    fn create_pipe_instance(first: bool) -> HANDLE {
        let name = wide(PIPE_NAME);
        let open_mode = if first {
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            PIPE_ACCESS_INBOUND
        };
        unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                0,
                4096,
                0,
                ptr::null_mut(),
            )
        }
    }

    pub fn claim_or_forward(path: Option<&std::path::Path>) -> Option<Receiver<Handoff>> {
        let handle = create_pipe_instance(true);

        if handle == INVALID_HANDLE_VALUE {
            if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                // A primary instance already exists and holds the first-pipe
                // flag: hand our file to it and get out of the way.
                forward_and_exit(path);
            }
            // Any other failure should be rare (e.g. the OS pipe limit hit).
            // Fail open — behave like before this fix rather than refusing to
            // launch at all.
            return Some(mpsc::channel().1);
        }

        // We're primary. Hand the HANDLE to a background thread that accepts
        // connections for the rest of this process's life. HANDLE is a raw
        // pointer (not Send); round-tripping it through usize is the standard
        // way to move a Windows handle across a thread boundary.
        let (tx, rx) = mpsc::channel();
        let handle_bits = handle as usize;
        thread::Builder::new()
            .name("rustednote-single-instance".into())
            .spawn(move || server_loop(handle_bits as HANDLE, tx))
            .expect("failed to start single-instance listener thread");

        Some(rx)
    }

    /// Runs for the lifetime of the primary process: accept one client, read
    /// its message, hand the connection to the app (a path to open, or just
    /// "someone launched me again" if it had none), then open a fresh pipe
    /// instance and wait for the next client.
    fn server_loop(first_handle: HANDLE, tx: mpsc::Sender<Handoff>) {
        let mut handle = first_handle;
        loop {
            let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) != 0 };
            // ERROR_PIPE_CONNECTED means a client connected in the window
            // between creating this instance and calling ConnectNamedPipe —
            // that's success too, not a failure.
            if connected || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
                // Sent unconditionally (even an empty message): every
                // relaunch should bring the window forward, not just the
                // ones that also named a file.
                let _ = tx.send(read_one_message(handle));
            }
            unsafe {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
            }

            handle = create_pipe_instance(false);
            if handle == INVALID_HANDLE_VALUE {
                // Should not happen in practice; stop rather than spin on a
                // permanently broken pipe.
                break;
            }
        }
    }

    /// Reads one client's whole message (it writes once, then closes its end,
    /// which is what ends this loop) and decodes it as a path. `None` for an
    /// empty message — a launch with no file, just asking to be focused.
    fn read_one_message(handle: HANDLE) -> Option<PathBuf> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let mut read: DWORD = 0;
            let ok = unsafe {
                ReadFile(
                    handle,
                    chunk.as_mut_ptr() as *mut _,
                    chunk.len() as DWORD,
                    &mut read,
                    ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..read as usize]);
            if buf.len() > MAX_MESSAGE {
                return None;
            }
        }
        if buf.is_empty() {
            None
        } else {
            Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
        }
    }

    /// Secondary instance: hand our file (if any) to the primary and quit.
    /// Connects even with no file, so a plain relaunch still brings the
    /// running window to the front rather than doing nothing. Best effort — a
    /// primary that's mid-shutdown just means this is a no-op, not a crash.
    /// Never returns.
    fn forward_and_exit(path: Option<&std::path::Path>) -> ! {
        if let Ok(mut client) = OpenOptions::new().write(true).open(PIPE_NAME) {
            if let Some(path) = path {
                let _ = client.write_all(path.to_string_lossy().as_bytes());
            }
            let _ = client.flush();
        }
        std::process::exit(0);
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn claim_or_forward(_path: Option<&std::path::Path>) -> Option<Receiver<Handoff>> {
        Some(std::sync::mpsc::channel().1)
    }
}
