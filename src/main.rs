//! rustednote — a cross-platform plain-text editor built with GPUI.
//!
//! Phase 2: persisted preferences, live theme switching, a system-font picker,
//! and Ctrl +/-/0 zoom, on top of the Phase 1 editable text view.

#![allow(dead_code)]
// GUI app: don't spawn a console window on Windows.
#![windows_subsystem = "windows"]

mod app;
mod bookmarks;
mod editor;
mod fonts;
mod integration;
mod prefs;
mod recent;
mod session;
mod single_instance;
mod state;
mod storage;
mod tabs;
mod text_field;
mod theme;
mod winutil;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions,
};

use app::{
    BookmarkCurrent, CloseSettings, CloseTab, CycleTheme, FindNext, FindPrev, NewTab, NextTab,
    OpenFile, PaletteConfirm, PaletteDown, PaletteUp, PrevTab, ReplaceAll, ReplaceOne, ShowFind,
    ShowPalette, ShowReplace, ToggleBookmarks, ToggleFocus, ToggleSettings, ToggleTabs, Workspace,
    ZoomIn, ZoomOut, ZoomReset,
};
use editor::view::{
    Backspace, Copy, Cut, CtrlLineDown, CtrlLineUp, Delete, DeleteWordNext, DeleteWordPrev,
    DocEnd, DocStart, Down, End, Home, Indent, Left, Newline, Outdent, Paste, Redo, Right, Save,
    SaveAs, SelectAll, SelectDocEnd, SelectDocStart, SelectDown, SelectLeft, SelectRight,
    SelectUp, SelectWordLeft, SelectWordRight, Undo, Up, WordLeft, WordRight,
};
use prefs::Preferences;
use text_field::{TfBackspace, TfDelete, TfEnd, TfHome, TfLeft, TfRight};

/// Ctrl on Windows/Linux, Cmd on macOS.
#[cfg(target_os = "macos")]
const MOD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "ctrl";

fn editor_keybindings() -> Vec<KeyBinding> {
    let ctx = Some("Editor");
    vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new(&format!("{MOD}-home"), DocStart, ctx),
        KeyBinding::new(&format!("{MOD}-end"), DocEnd, ctx),
        KeyBinding::new(&format!("{MOD}-shift-home"), SelectDocStart, ctx),
        KeyBinding::new(&format!("{MOD}-shift-end"), SelectDocEnd, ctx),
        // Walks line boundaries: end/start of the line above (repeat to keep
        // climbing), start/end of the line below.
        KeyBinding::new(&format!("{MOD}-up"), CtrlLineUp, ctx),
        KeyBinding::new(&format!("{MOD}-down"), CtrlLineDown, ctx),
        KeyBinding::new("enter", Newline, ctx),
        KeyBinding::new("tab", Indent, ctx),
        KeyBinding::new("shift-tab", Outdent, ctx),
        KeyBinding::new(&format!("{MOD}-left"), WordLeft, ctx),
        KeyBinding::new(&format!("{MOD}-right"), WordRight, ctx),
        KeyBinding::new(&format!("{MOD}-shift-left"), SelectWordLeft, ctx),
        KeyBinding::new(&format!("{MOD}-shift-right"), SelectWordRight, ctx),
        KeyBinding::new(&format!("{MOD}-backspace"), DeleteWordPrev, ctx),
        KeyBinding::new(&format!("{MOD}-delete"), DeleteWordNext, ctx),
        KeyBinding::new(&format!("{MOD}-a"), SelectAll, ctx),
        KeyBinding::new(&format!("{MOD}-c"), Copy, ctx),
        KeyBinding::new(&format!("{MOD}-x"), Cut, ctx),
        KeyBinding::new(&format!("{MOD}-v"), Paste, ctx),
        KeyBinding::new(&format!("{MOD}-z"), Undo, ctx),
        KeyBinding::new(&format!("{MOD}-y"), Redo, ctx),
        KeyBinding::new(&format!("{MOD}-shift-z"), Redo, ctx),
        KeyBinding::new(&format!("{MOD}-s"), Save, ctx),
        KeyBinding::new(&format!("{MOD}-shift-s"), SaveAs, ctx),
    ]
}

fn workspace_keybindings() -> Vec<KeyBinding> {
    // Global (context-less) so they work regardless of focus.
    vec![
        KeyBinding::new(&format!("{MOD}-="), ZoomIn, None),
        KeyBinding::new(&format!("{MOD}-+"), ZoomIn, None),
        KeyBinding::new(&format!("{MOD}--"), ZoomOut, None),
        KeyBinding::new(&format!("{MOD}-0"), ZoomReset, None),
        KeyBinding::new(&format!("{MOD}-,"), ToggleSettings, None),
        KeyBinding::new("escape", CloseSettings, None),
        KeyBinding::new(&format!("{MOD}-t"), NewTab, None),
        KeyBinding::new(&format!("{MOD}-n"), NewTab, None),
        KeyBinding::new(&format!("{MOD}-w"), CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new(&format!("{MOD}-o"), OpenFile, None),
        KeyBinding::new(&format!("{MOD}-b"), ToggleBookmarks, None),
        KeyBinding::new(&format!("{MOD}-d"), BookmarkCurrent, None),
        KeyBinding::new(&format!("{MOD}-shift-l"), ToggleTabs, None),
        KeyBinding::new(&format!("{MOD}-shift-t"), CycleTheme, None),
        KeyBinding::new("f11", ToggleFocus, None),
        KeyBinding::new(&format!("{MOD}-f"), ShowFind, None),
        KeyBinding::new(&format!("{MOD}-h"), ShowReplace, None),
        KeyBinding::new(&format!("{MOD}-shift-p"), ShowPalette, None),
    ]
}

/// Bindings for the Find bar, command palette, and text fields (scoped by the
/// key contexts set on those elements).
fn overlay_keybindings() -> Vec<KeyBinding> {
    vec![
        // Find bar (container key_context = "Find").
        KeyBinding::new("enter", FindNext, Some("Find")),
        KeyBinding::new("shift-enter", FindPrev, Some("Find")),
        KeyBinding::new("f3", FindNext, Some("Find")),
        KeyBinding::new("shift-f3", FindPrev, Some("Find")),
        KeyBinding::new(&format!("{MOD}-enter"), ReplaceOne, Some("Find")),
        KeyBinding::new(&format!("{MOD}-shift-enter"), ReplaceAll, Some("Find")),
        // Command palette (container key_context = "Palette").
        KeyBinding::new("enter", PaletteConfirm, Some("Palette")),
        KeyBinding::new("up", PaletteUp, Some("Palette")),
        KeyBinding::new("down", PaletteDown, Some("Palette")),
        // Single-line text fields (key_context = "TextField").
        KeyBinding::new("backspace", TfBackspace, Some("TextField")),
        KeyBinding::new("delete", TfDelete, Some("TextField")),
        KeyBinding::new("left", TfLeft, Some("TextField")),
        KeyBinding::new("right", TfRight, Some("TextField")),
        KeyBinding::new("home", TfHome, Some("TextField")),
        KeyBinding::new("end", TfEnd, Some("TextField")),
    ]
}

/// Handles the headless shell-integration switches, if one was passed.
/// Returns true when the process has done its job and should exit without
/// opening a window — the MSI uninstaller calls `--unregister-shell` this way
/// (see PACKAGING.md), and it must not pop a UI mid-uninstall.
fn run_shell_switch() -> bool {
    let Some(flag) = std::env::args().skip(1).find(|a| a.starts_with("--")) else {
        return false;
    };
    match flag.as_str() {
        "--register-shell" => {
            std::process::exit(i32::from(integration::register().is_err()));
        }
        "--unregister-shell" => {
            std::process::exit(i32::from(integration::unregister().is_err()));
        }
        _ => false,
    }
}

fn main() {
    if run_shell_switch() {
        return;
    }

    // Self-repair: drop an "Open with" entry pointing at a RustedNote that no
    // longer exists, so a previous install that was removed without cleaning up
    // doesn't leave a dead entry in the shell's context menu forever.
    integration::prune_stale();

    // File passed on the command line (e.g. Windows "Open with…"). Skips
    // any `--flag`, so a switch never gets mistaken for a path. Parsed here,
    // before touching gpui at all, so the single-instance check below can
    // hand it to an already-running RustedNote and exit without ever
    // spinning up a window of its own.
    let initial_file = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file());

    // Single-instance: if RustedNote is already running, hand this file over
    // to it and stop here instead of opening a second window for it — see
    // single_instance.rs. Without this, double-clicking a second file while
    // the app was already open spawned a whole separate process/window, and
    // closing the "wrong" one (or just the original) could lose whatever was
    // typed into an Untitled tab there.
    //
    // On Windows, losing that race exits the process directly inside
    // `claim_or_forward` and this line never returns; `incoming_files`
    // otherwise delivers paths forwarded by any *later* launch.
    let Some(incoming_files) = single_instance::claim_or_forward(initial_file.as_deref()) else {
        return;
    };

    Application::new().run(move |cx: &mut App| {
        // Load persisted preferences (or defaults) into the global store.
        let prefs = storage::load_or_default::<Preferences>(prefs::PREFS_FILE);
        cx.set_global(prefs);
        cx.set_global(state::UiState::default());

        cx.bind_keys(editor_keybindings());
        cx.bind_keys(workspace_keybindings());
        cx.bind_keys(overlay_keybindings());

        let bounds = Bounds::centered(None, size(px(960.0), px(680.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Custom titlebar: nav shares the strip with the window
                    // controls. Dragging uses window_control_area(Drag); the
                    // min/max/close buttons use explicit window methods.
                    titlebar: Some(TitlebarOptions {
                        title: Some("RustedNote".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    // Keep the window large enough that the titlebar never overlaps.
                    window_min_size: Some(size(px(360.0), px(360.0))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Workspace::new(cx, initial_file)),
            )
            .expect("failed to open window");

        let ws_entity = window
            .update(cx, |ws, window, cx| {
                let handle = ws.active_editor().read(cx).focus_handle.clone();
                window.focus(&handle);
                // Re-render when the OS light/dark appearance changes (System theme).
                window
                    .observe_window_appearance(|window, _| window.refresh())
                    .detach();
                cx.entity()
            })
            .ok();

        // Persist the session (open tabs + unsaved content) whenever the
        // window closes, regardless of how it was closed (✕ button, Alt+F4,
        // taskbar, etc.) — this is the one hook that fires for all of them.
        if let Some(ws_entity) = ws_entity {
            cx.on_window_closed(move |cx| {
                ws_entity.update(cx, |ws, cx| ws.save_session(cx));
            })
            .detach();
        }

        // Files handed over by a *later* launch (Windows "Open with…" while
        // we're already running — see single_instance.rs) arrive on this
        // channel; open each as a new tab in this same window and bring it to
        // the front, rather than the second launch opening its own window.
        // Polled on a short timer rather than woken directly, since the
        // sending side is a plain OS thread outside gpui's own executors.
        cx.spawn(async move |cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            while let Ok(handoff) = incoming_files.try_recv() {
                let opened = window.update(cx, |ws, window, cx| {
                    if let Some(path) = handoff {
                        ws.open_path(path, window, cx);
                    }
                    window.activate_window();
                });
                if opened.is_err() {
                    // The window is gone; nothing left to hand files to.
                    return;
                }
            }
        })
        .detach();

        cx.activate(true);
    });
}
