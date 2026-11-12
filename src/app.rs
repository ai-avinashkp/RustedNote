//! The workspace: the window root. Hosts the menu bar, tab strip, active
//! editor, and the settings / bookmark / file-menu overlays. Owns the open
//! documents, the shared [`Preferences`] global, the bookmark store, and the
//! recent-files history — persisting each on change.

use std::path::PathBuf;

use gpui::{
    actions, div, prelude::*, px, rgb, rgba, App, Context, CursorStyle, Entity, FocusHandle,
    Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollHandle, SharedString, Window,
};

use crate::bookmarks::{Bookmarks, BOOKMARKS_FILE};
use crate::editor::buffer::Buffer;
use crate::editor::view::{os_is_dark, EditorView};
use crate::fonts;
use crate::integration;
use crate::prefs::{Preferences, TabDir, ThemeChoice, PREFS_FILE};
use crate::recent::{RecentFiles, RECENT_FILE};
use crate::session::{Session, SessionTab, SESSION_FILE};
use crate::state::UiState;
use crate::storage;
use crate::tabs;
use crate::text_field::TextField;
use crate::theme::Palette;

/// Store preferences in GPUI's global state so any view can read them live.
impl gpui::Global for Preferences {}

// Toolbar/menu buttons: dark background, light text, everywhere (per request).
const BTN_BG: u32 = 0x2d2d30;
const BTN_HOVER: u32 = 0x3f3f46;
const BTN_TEXT: u32 = 0xf5f5f5;

actions!(
    workspace,
    [
        ZoomIn, ZoomOut, ZoomReset, ToggleSettings, CloseSettings, NewTab, CloseTab, NextTab,
        PrevTab, OpenFile, ToggleTabs, ToggleBookmarks, BookmarkCurrent, CycleTheme, ToggleFocus,
        ShowFind, ShowReplace, FindNext, FindPrev, ReplaceOne, ReplaceAll, ShowPalette, PaletteUp,
        PaletteDown, PaletteConfirm,
    ]
);

/// Commands exposed in the command palette: (label, id). `id` is matched in
/// `run_command`.
const COMMANDS: &[(&str, &str)] = &[
    ("New Tab", "new-tab"),
    ("Open File…", "open"),
    ("Save", "save"),
    ("Save As…", "save-as"),
    ("Close Tab", "close-tab"),
    ("Next Tab", "next-tab"),
    ("Previous Tab", "prev-tab"),
    ("Find…", "find"),
    ("Replace…", "replace"),
    ("Toggle Tab Layout", "toggle-tabs"),
    ("Bookmarks", "bookmarks"),
    ("Bookmark Current File", "bookmark"),
    ("Font Family…", "fonts"),
    ("Settings", "settings"),
    ("Focus Mode", "focus"),
    ("Zoom In", "zoom-in"),
    ("Zoom Out", "zoom-out"),
    ("Reset Zoom", "zoom-reset"),
    ("Cycle Theme", "cycle-theme"),
];

/// Shown in Settings so users can discover every binding. Mirrors `main.rs`.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Undo", "Ctrl+Z"),
    ("Redo", "Ctrl+Y  ·  Ctrl+Shift+Z"),
    ("Cut / Copy / Paste", "Ctrl+X / Ctrl+C / Ctrl+V"),
    ("Select all", "Ctrl+A"),
    ("Indent / Outdent line", "Tab / Shift+Tab"),
    ("Start / end of note", "Ctrl+Home / Ctrl+End"),
    ("Select to start / end of note", "Ctrl+Shift+Home / Ctrl+Shift+End"),
    ("Start / end of line", "Home / End"),
    ("Select up / down a line", "Shift+Up / Shift+Down"),
    ("Walk line boundaries", "Ctrl+Up / Ctrl+Down"),
    ("Save / Save As", "Ctrl+S / Ctrl+Shift+S"),
    ("Open file", "Ctrl+O"),
    ("New tab / Close tab", "Ctrl+T / Ctrl+W"),
    ("Next / Previous tab", "Ctrl+Tab / Ctrl+Shift+Tab"),
    ("Toggle tab layout", "Ctrl+Shift+L"),
    ("Find / Replace", "Ctrl+F / Ctrl+H"),
    ("Find next / previous", "Enter / Shift+Enter  (F3)"),
    ("Replace one / all", "Ctrl+Enter / Ctrl+Shift+Enter"),
    ("Command palette", "Ctrl+Shift+P"),
    ("Bookmarks / Bookmark file", "Ctrl+B / Ctrl+D"),
    ("Zoom in / out / reset", "Ctrl+= / Ctrl+- / Ctrl+0"),
    ("Zoom (mouse wheel)", "Ctrl+Scroll"),
    ("Cycle theme", "Ctrl+Shift+T"),
    ("Focus mode", "F11  (Esc to exit)"),
    ("Move / select by word", "Ctrl+Left/Right  ·  Ctrl+Shift+Left/Right"),
    ("Delete word", "Ctrl+Backspace / Ctrl+Delete"),
];

const WELCOME: &str = "Welcome to RustedNote.\n\nStart typing — this is a real, editable, \
plain-text buffer.\n\nTry it:\n  • File menu, or Ctrl+T new tab / Ctrl+W close / Ctrl+Tab cycle\n \
 • Ctrl+, settings · Ctrl+B bookmarks · Ctrl+D bookmark this file\n  • Ctrl +/- zoom · Ctrl+S \
save · Ctrl+O open · F11 focus mode\n";

pub struct Workspace {
    editors: Vec<Entity<EditorView>>,
    active: usize,
    focus_handle: FocusHandle,
    /// Focus target for open modals, so keystrokes can't reach the editor.
    modal_focus: FocusHandle,
    show_settings: bool,
    show_bookmarks: bool,
    show_file_menu: bool,
    show_recent_submenu: bool,
    show_fonts: bool,
    focus_mode: bool,
    show_find: bool,
    replace_mode: bool,
    show_palette: bool,
    palette_selected: usize,
    /// Tab index waiting on a "save before closing?" answer, if any.
    confirm_close: Option<usize>,
    /// Titlebar mouse-down position (Some while pressed); drag starts once the
    /// pointer moves past a threshold, so a double-click can maximize instead.
    titlebar_press: Option<Point<Pixels>>,
    /// Our own maximized-state tracking (IsZoomed is unreliable for this window).
    maximized: bool,
    find_field: Entity<TextField>,
    replace_field: Entity<TextField>,
    palette_field: Entity<TextField>,
    fonts: Vec<SharedString>,
    bookmarks: Bookmarks,
    recent: RecentFiles,
    /// Outcome of the last "Open with" register/remove, shown in Settings.
    open_with_status: Option<SharedString>,
    font_scroll: ScrollHandle,
    tab_scroll: ScrollHandle,
    bookmark_scroll: ScrollHandle,
    palette_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    bmk_panel_scroll: ScrollHandle,
    /// Last known viewport size (px), refreshed each render for responsiveness.
    vp_width: f32,
    vp_height: f32,
}

impl Workspace {
    /// `initial_file` is a path passed on the command line (e.g. Windows
    /// "Open with…"); when readable, it opens as the active tab alongside any
    /// restored session tabs.
    pub fn new(cx: &mut Context<Self>, initial_file: Option<PathBuf>) -> Self {
        // Recent files: prune paths whose files were deleted, then persist.
        let mut recent = storage::load_or_default::<RecentFiles>(RECENT_FILE);
        if recent.prune_missing() {
            let _ = storage::save(RECENT_FILE, &recent);
        }

        let restore_session = cx.global::<Preferences>().restore_session;
        let mut editors: Vec<Entity<EditorView>> = Vec::new();
        let mut active_idx = 0usize;
        // True once a session was found on disk: it means this isn't a first
        // run, so an empty workspace should stay empty rather than re-showing
        // the welcome text.
        let mut had_session = false;

        if restore_session {
            let mut session = storage::load_or_default::<Session>(SESSION_FILE);
            had_session = !session.tabs.is_empty();
            // Ghost check: drop entries whose backing file no longer exists and
            // has no unsaved content to fall back on, plus blank Untitled
            // scratch tabs, then re-persist.
            let pruned = session.prune_missing() | session.prune_blank_untitled();
            if pruned {
                let _ = storage::save(SESSION_FILE, &session);
            }
            for tab in &session.tabs {
                let (content, path, had_unsaved) = if let Some(text) = &tab.unsaved_content {
                    (text.clone(), tab.path.clone(), true)
                } else if let Some(p) = &tab.path {
                    match std::fs::read(p) {
                        Ok(bytes) => {
                            let (text, _, _) = encoding_rs::UTF_8.decode(&bytes);
                            (text.replace("\r\n", "\n"), Some(p.clone()), false)
                        }
                        Err(_) => continue,
                    }
                } else {
                    continue;
                };
                editors.push(cx.new(|cx| {
                    let mut buf = Buffer::from_str(&content);
                    if had_unsaved && !content.is_empty() {
                        // Harmless no-op edit: flips Buffer's dirty flag on so
                        // the restored tab correctly shows as unsaved.
                        buf.replace(0..0, "");
                    }
                    EditorView::new(cx, buf, path.clone())
                }));
            }
            if !editors.is_empty() {
                active_idx = session.active.min(editors.len() - 1);
            }
        }

        if let Some(path) = initial_file {
            // Explicit "open this file" (e.g. Windows "Open with…") is added
            // on top of the restored session rather than replacing it —
            // otherwise the session (and any unsaved Untitled text in it) is
            // overwritten with just this one tab when the window next closes.
            // If the file is already among the restored tabs, focus that one.
            let opened = if let Some(i) =
                editors.iter().position(|e| e.read(cx).path() == Some(&path))
            {
                active_idx = i;
                true
            } else if let Ok(bytes) = std::fs::read(&path) {
                let (text, _, _) = encoding_rs::UTF_8.decode(&bytes);
                let text = text.replace("\r\n", "\n");
                editors.push(
                    cx.new(|cx| EditorView::new(cx, Buffer::from_str(&text), Some(path.clone()))),
                );
                active_idx = editors.len() - 1;
                true
            } else {
                false
            };
            if opened {
                recent.record(&path);
                let _ = storage::save(RECENT_FILE, &recent);
            }
        }

        if editors.is_empty() {
            // A returning user gets a blank sheet; the welcome text is for the
            // very first launch only.
            let text = if had_session { "" } else { WELCOME };
            editors.push(cx.new(|cx| EditorView::new(cx, Buffer::from_str(text), None)));
            active_idx = 0;
        }
        for e in &editors {
            cx.observe(e, |_, _, cx| cx.notify()).detach();
        }

        let ts = cx.text_system();
        let fonts = fonts::system_fonts(&ts);
        let bookmarks = storage::load_or_default::<Bookmarks>(BOOKMARKS_FILE);

        let find_field = cx.new(|cx| TextField::new(cx, "Find"));
        let replace_field = cx.new(|cx| TextField::new(cx, "Replace with"));
        let palette_field = cx.new(|cx| TextField::new(cx, "Type a command…"));
        // Re-render (live filter / match state) as the fields change.
        for f in [&find_field, &palette_field] {
            cx.observe(f, |_, _, cx| cx.notify()).detach();
        }

        Workspace {
            editors,
            active: active_idx,
            focus_handle: cx.focus_handle(),
            modal_focus: cx.focus_handle(),
            show_settings: false,
            show_bookmarks: false,
            show_file_menu: false,
            show_recent_submenu: false,
            show_fonts: false,
            focus_mode: false,
            show_find: false,
            replace_mode: false,
            show_palette: false,
            palette_selected: 0,
            confirm_close: None,
            titlebar_press: None,
            maximized: false,
            find_field,
            replace_field,
            palette_field,
            fonts,
            bookmarks,
            recent,
            open_with_status: None,
            font_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            bookmark_scroll: ScrollHandle::new(),
            palette_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            bmk_panel_scroll: ScrollHandle::new(),
            vp_width: 960.,
            vp_height: 680.,
        }
    }

    /// Height for an overlay's scrollable inner list, clamped to the viewport.
    fn list_height(&self, fraction: f32, min: f32, max: f32) -> gpui::Pixels {
        px((self.vp_height * fraction).clamp(min, max))
    }

    pub fn active_editor(&self) -> &Entity<EditorView> {
        &self.editors[self.active]
    }

    // --- persistence ------------------------------------------------------

    fn persist(&self, cx: &mut Context<Self>) {
        let prefs = cx.global::<Preferences>().clone();
        if let Err(e) = storage::save(PREFS_FILE, &prefs) {
            eprintln!("could not save preferences: {e}");
        }
    }

    fn persist_bookmarks(&self) {
        if let Err(e) = storage::save(BOOKMARKS_FILE, &self.bookmarks) {
            eprintln!("could not save bookmarks: {e}");
        }
    }

    fn persist_recent(&self) {
        if let Err(e) = storage::save(RECENT_FILE, &self.recent) {
            eprintln!("could not save recent files: {e}");
        }
    }

    /// Snapshot every open tab (path + live buffer text for anything unsaved
    /// or untitled) to `session.json`. Called when the window closes, so
    /// "open where you left off" survives — including unsaved Untitled text.
    pub fn save_session(&self, cx: &mut Context<Self>) {
        let tabs: Vec<SessionTab> = self
            .editors
            .iter()
            .map(|e| {
                let ed = e.read(cx);
                let path = ed.path().cloned();
                let unsaved_content = if ed.is_dirty() || path.is_none() {
                    Some(ed.buffer_text())
                } else {
                    None
                };
                SessionTab { path, unsaved_content }
            })
            .collect();
        let session = Session { tabs, active: self.active };
        if let Err(e) = storage::save(SESSION_FILE, &session) {
            eprintln!("could not save session: {e}");
        }
    }

    fn observe_editor(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        cx.observe(editor, |_, _, cx| cx.notify()).detach();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<UiState, _>(|s, _| s.modal_open = false);
        if let Some(editor) = self.editors.get(self.active) {
            let handle = editor.read(cx).focus_handle.clone();
            window.focus(&handle);
        }
    }

    /// Move keyboard focus onto the modal focus target (off the editor) AND set
    /// the global modal flag, so the editor refuses input while a modal is open.
    fn blur_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.modal_focus);
        cx.update_global::<UiState, _>(|s, _| s.modal_open = true);
    }

    // --- zoom / settings / theme -----------------------------------------

    fn apply_zoom_in(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.zoom_in());
        self.persist(cx);
        cx.notify();
    }

    fn apply_zoom_out(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.zoom_out());
        self.persist(cx);
        cx.notify();
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_zoom_in(cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_zoom_out(cx);
    }

    fn zoom_reset(&mut self, _: &ZoomReset, _: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.zoom_reset());
        self.persist(cx);
        cx.notify();
    }

    fn toggle_settings(&mut self, _: &ToggleSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings = !self.show_settings;
        self.show_bookmarks = false;
        self.show_fonts = false;
        self.show_file_menu = false;
        // A message from a previous visit shouldn't greet the next one.
        self.open_with_status = None;
        if self.show_settings {
            self.blur_editor(window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn close_settings(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
        // Escape on the close prompt means "cancel the close", not "dismiss
        // every panel" — answer that one and stop there.
        if self.confirm_close.is_some() {
            self.confirm_close_cancel(window, cx);
            return;
        }
        let any = self.show_settings
            || self.show_bookmarks
            || self.show_file_menu
            || self.show_fonts
            || self.focus_mode
            || self.show_find
            || self.show_palette;
        if any {
            self.show_settings = false;
            self.show_bookmarks = false;
            self.show_file_menu = false;
            self.show_recent_submenu = false;
            self.show_fonts = false;
            self.focus_mode = false;
            self.show_find = false;
            self.show_palette = false;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn set_theme(&mut self, theme: ThemeChoice, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.theme = theme);
        self.persist(cx);
        cx.notify();
    }

    fn cycle_theme(&mut self, _: &CycleTheme, _: &mut Window, cx: &mut Context<Self>) {
        let next = cx.global::<Preferences>().theme.cycle_primary();
        self.set_theme(next, cx);
    }

    fn set_word_wrap(&mut self, value: bool, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.word_wrap = value);
        self.persist(cx);
        cx.notify();
    }

    fn set_show_line_numbers(&mut self, value: bool, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.show_line_numbers = value);
        self.persist(cx);
        cx.notify();
    }

    fn set_restore_session(&mut self, value: bool, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.restore_session = value);
        self.persist(cx);
        cx.notify();
    }

    /// Adds or removes RustedNote from the OS "Open with" menu for plain-text
    /// files. Both directions are per-user and reversible — see `integration`.
    fn set_open_with(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let result = if enabled {
            integration::register()
        } else {
            integration::unregister()
        };

        self.open_with_status = Some(match result {
            Ok(()) if enabled => "RustedNote now appears in “Open with” for text files.".into(),
            Ok(()) => "Removed RustedNote from “Open with”.".into(),
            Err(error) => format!("Could not change the file association: {error}").into(),
        });
        cx.notify();
    }

    fn set_font(&mut self, name: String, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.font_family = name);
        self.persist(cx);
        cx.notify();
    }

    fn font_bigger(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.font_bigger());
        self.persist(cx);
        cx.notify();
    }

    fn font_smaller(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.font_smaller());
        self.persist(cx);
        cx.notify();
    }

    fn toggle_focus(&mut self, _: &ToggleFocus, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_mode = !self.focus_mode;
        cx.notify();
    }

    // --- find / replace ---------------------------------------------------

    fn show_find(&mut self, _: &ShowFind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(false, window, cx);
    }

    fn show_replace(&mut self, _: &ShowReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(true, window, cx);
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.show_find = true;
        self.replace_mode = replace;
        // Seed the query with the current selection, if any.
        let sel = self.active_editor().read(cx).selected_text();
        if !sel.is_empty() && !sel.contains('\n') {
            self.find_field.update(cx, |f, cx| f.set_text(&sel, cx));
        }
        let handle = self.find_field.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    fn do_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let query = self.find_field.read(cx).text().to_string();
        if query.is_empty() {
            return;
        }
        let ed = self.active_editor().clone();
        let text = ed.read(cx).buffer_text();
        let starts: Vec<usize> = text.match_indices(&query).map(|(i, _)| i).collect();
        if starts.is_empty() {
            return;
        }
        let target = if forward {
            let from = ed.read(cx).selection_end();
            starts
                .iter()
                .copied()
                .find(|&s| s >= from)
                .unwrap_or(starts[0])
        } else {
            let from = ed.read(cx).selection_start();
            starts
                .iter()
                .rev()
                .copied()
                .find(|&s| s < from)
                .unwrap_or(*starts.last().unwrap())
        };
        ed.update(cx, |e, cx| e.select_range(target, target + query.len(), cx));
        cx.notify();
    }

    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.do_find(true, cx);
    }

    fn find_prev(&mut self, _: &FindPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.do_find(false, cx);
    }

    fn replace_one(&mut self, _: &ReplaceOne, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.find_field.read(cx).text().to_string();
        if query.is_empty() {
            return;
        }
        let replacement = self.replace_field.read(cx).text().to_string();
        let ed = self.active_editor().clone();
        // If the current selection is the query, replace it; then advance.
        if ed.read(cx).selected_text() == query {
            ed.update(cx, |e, cx| e.replace_current_selection(&replacement, cx));
        }
        self.do_find(true, cx);
    }

    fn replace_all(&mut self, _: &ReplaceAll, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.find_field.read(cx).text().to_string();
        if query.is_empty() {
            return;
        }
        let replacement = self.replace_field.read(cx).text().to_string();
        let ed = self.active_editor().clone();
        let text = ed.read(cx).buffer_text();
        if !text.contains(&query) {
            return;
        }
        let new_text = text.replace(&query, &replacement);
        ed.update(cx, |e, cx| e.set_all(&new_text, cx));
        cx.notify();
    }

    // --- command palette --------------------------------------------------

    fn filtered_commands(&self, cx: &Context<Self>) -> Vec<(&'static str, &'static str)> {
        let q = self.palette_field.read(cx).text().to_lowercase();
        COMMANDS
            .iter()
            .filter(|(label, _)| q.is_empty() || label.to_lowercase().contains(&q))
            .copied()
            .collect()
    }

    fn show_palette(&mut self, _: &ShowPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.show_palette = true;
        self.palette_selected = 0;
        let field = self.palette_field.clone();
        field.update(cx, |f, cx| f.clear(cx));
        let handle = field.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    fn palette_up(&mut self, _: &PaletteUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_selected > 0 {
            self.palette_selected -= 1;
            cx.notify();
        }
    }

    fn palette_down(&mut self, _: &PaletteDown, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.filtered_commands(cx).len();
        if n > 0 && self.palette_selected + 1 < n {
            self.palette_selected += 1;
            cx.notify();
        }
    }

    fn palette_confirm(&mut self, _: &PaletteConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let cmds = self.filtered_commands(cx);
        if let Some((_, id)) = cmds.get(self.palette_selected).copied() {
            self.show_palette = false;
            self.run_command(id, window, cx);
        }
    }

    fn run_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "new-tab" => self.new_tab(&NewTab, window, cx),
            "open" => self.open_file(&OpenFile, window, cx),
            "save" => {
                let ed = self.active_editor().clone();
                ed.update(cx, |e, cx| e.perform_save(cx));
            }
            "save-as" => {
                let ed = self.active_editor().clone();
                ed.update(cx, |e, cx| e.perform_save_as(cx));
            }
            "close-tab" => self.close_tab(&CloseTab, window, cx),
            "next-tab" => self.next_tab(&NextTab, window, cx),
            "prev-tab" => self.prev_tab(&PrevTab, window, cx),
            "find" => self.open_find(false, window, cx),
            "replace" => self.open_find(true, window, cx),
            "toggle-tabs" => self.toggle_tabs(&ToggleTabs, window, cx),
            "bookmarks" => self.toggle_bookmarks(&ToggleBookmarks, window, cx),
            "bookmark" => self.bookmark_current(&BookmarkCurrent, window, cx),
            "fonts" => self.open_fonts(window, cx),
            "settings" => self.toggle_settings(&ToggleSettings, window, cx),
            "focus" => self.toggle_focus(&ToggleFocus, window, cx),
            "zoom-in" => self.apply_zoom_in(cx),
            "zoom-out" => self.apply_zoom_out(cx),
            "zoom-reset" => self.zoom_reset(&ZoomReset, window, cx),
            "cycle-theme" => self.cycle_theme(&CycleTheme, window, cx),
            _ => {}
        }
        cx.notify();
    }

    // --- tabs -------------------------------------------------------------

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| EditorView::new(cx, Buffer::default(), None));
        self.observe_editor(&editor, cx);
        self.editors.push(editor);
        self.active = self.editors.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.editors.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Whether closing this tab would throw away writing that exists nowhere
    /// else. An untitled note with content has no file to fall back on, so it
    /// is the one case worth interrupting the user over.
    fn needs_close_confirmation(&self, index: usize, cx: &Context<Self>) -> bool {
        self.editors.get(index).is_some_and(|editor| {
            let editor = editor.read(cx);
            editor.path().is_none() && editor.is_dirty()
        })
    }

    fn close_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.editors.len() {
            return;
        }
        if self.needs_close_confirmation(index, cx) {
            self.confirm_close = Some(index);
            self.blur_editor(window, cx);
            cx.notify();
            return;
        }
        self.force_close_index(index, window, cx);
    }

    /// Close without asking — the answer has already been given, or there was
    /// nothing to lose.
    fn force_close_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.editors.len() {
            return;
        }
        if self.editors.len() == 1 {
            let editor = cx.new(|cx| EditorView::new(cx, Buffer::default(), None));
            self.observe_editor(&editor, cx);
            self.editors[0] = editor;
            self.active = 0;
        } else {
            self.editors.remove(index);
            self.active = tabs::active_after_remove(self.active, index, self.editors.len());
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_index(self.active, window, cx);
    }

    // --- "save before closing?" ------------------------------------------

    /// Save, then close if the save actually happened. Picking a file can still
    /// be cancelled in the OS dialog, and that means "never mind" — the tab
    /// stays open rather than being closed with the work unsaved.
    fn confirm_close_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.confirm_close.take() else {
            return;
        };
        let Some(editor) = self.editors.get(index).cloned() else {
            self.focus_active(window, cx);
            cx.notify();
            return;
        };
        editor.update(cx, |editor, cx| editor.perform_save(cx));
        if editor.read(cx).is_dirty() {
            self.focus_active(window, cx);
        } else {
            self.force_close_index(index, window, cx);
        }
        cx.notify();
    }

    fn confirm_close_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.confirm_close.take() else {
            return;
        };
        self.force_close_index(index, window, cx);
        cx.notify();
    }

    fn confirm_close_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_close = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.active = tabs::next_index(self.active, self.editors.len());
        self.focus_active(window, cx);
        cx.notify();
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.active = tabs::prev_index(self.active, self.editors.len());
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_tabs(&mut self, _: &ToggleTabs, _: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| {
            p.tab_orientation = match p.tab_orientation {
                TabDir::Horizontal => TabDir::Vertical,
                TabDir::Vertical => TabDir::Horizontal,
            };
        });
        self.persist(cx);
        cx.notify();
    }

    // --- files & recent ---------------------------------------------------

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_menu = false;
        // Text-first, like classic Notepad.
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Text Documents (*.txt)", &["txt"])
            .add_filter("Text-like (*.md, *.log, *.ini, *.csv, *.json, *.xml)", &["md", "log", "ini", "csv", "json", "xml"])
            .add_filter("All Files (*.*)", &["*"])
            .pick_file()
        {
            self.open_path(path, window, cx);
        }
    }

    /// Open `path` as a tab — reusing an already-open one if it's already
    /// open — and make it active. `pub(crate)` so `main.rs` can call it when
    /// a later launch hands this process a file over the single-instance
    /// pipe (see `single_instance.rs`), not just from the in-app File menu.
    pub(crate) fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self
            .editors
            .iter()
            .position(|e| e.read(cx).path() == Some(&path))
        {
            self.active = i;
        } else {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let (text, _, _) = encoding_rs::UTF_8.decode(&bytes);
                    let buffer = Buffer::from_str(&text);
                    let editor = cx.new(|cx| EditorView::new(cx, buffer, Some(path.clone())));
                    self.observe_editor(&editor, cx);
                    self.editors.push(editor);
                    self.active = self.editors.len() - 1;
                }
                Err(e) => {
                    eprintln!("open failed: {e}");
                    return;
                }
            }
        }
        self.recent.record(&path);
        self.persist_recent();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn clear_recent(&mut self, cx: &mut Context<Self>) {
        self.recent.clear();
        self.persist_recent();
        cx.notify();
    }

    fn remove_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.recent.remove(&path);
        self.persist_recent();
        cx.notify();
    }

    // --- bookmarks --------------------------------------------------------

    fn bookmark_current(&mut self, _: &BookmarkCurrent, _: &mut Window, cx: &mut Context<Self>) {
        let path = self.editors[self.active].read(cx).path().cloned();
        if let Some(path) = path {
            self.bookmarks.add(&path);
            self.persist_bookmarks();
            cx.notify();
        }
    }

    fn toggle_bookmarks(
        &mut self,
        _: &ToggleBookmarks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_bookmarks = !self.show_bookmarks;
        self.show_settings = false;
        self.show_fonts = false;
        self.show_file_menu = false;
        if self.show_bookmarks {
            self.blur_editor(window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn open_fonts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_menu = false;
        self.show_recent_submenu = false;
        self.show_settings = false;
        self.show_bookmarks = false;
        self.show_fonts = true;
        self.blur_editor(window, cx);
        cx.notify();
    }

    fn open_bookmark(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.bookmarks.mark_opened(&path);
        self.persist_bookmarks();
        self.show_bookmarks = false;
        self.open_path(path, window, cx);
    }

    fn remove_bookmark(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.bookmarks.remove(&path);
        self.persist_bookmarks();
        cx.notify();
    }

    // --- buttons ----------------------------------------------------------

    /// A large button with dark background and light text (used across the UI).
    fn button(&self, label: impl Into<SharedString>) -> gpui::Div {
        div()
            .px_4()
            .py(px(6.))
            .rounded_md()
            .cursor_pointer()
            .whitespace_nowrap()
            .bg(rgb(BTN_BG))
            .text_color(rgb(BTN_TEXT))
            .hover(|s| s.bg(rgb(BTN_HOVER)))
            .child(label.into())
    }

    // --- rendering: titlebar ---------------------------------------------

    /// Compact titlebar button: dark bg, light text, tight vertical padding.
    fn tb_button(&self, label: impl Into<SharedString>) -> gpui::Div {
        div()
            .px_3()
            .py(px(2.))
            .rounded_md()
            .cursor_pointer()
            .whitespace_nowrap()
            .bg(rgb(BTN_BG))
            .text_color(rgb(BTN_TEXT))
            .hover(|s| s.bg(rgb(BTN_HOVER)))
            .child(label.into())
    }

    /// A simple, dependency-free app logo: a small light-gray rounded square
    /// with a black "N". Fixed colors, so it reads the same in every theme.
    fn logo(&self, _palette: Palette) -> gpui::Div {
        div()
            .w(px(20.))
            .h(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(0xd3d3d3))
            .text_color(rgb(0x000000))
            .text_size(px(12.))
            .child("N")
    }

    /// Custom window-control buttons (min / max / close). These use EXPLICIT
    /// window methods (reliable on Windows) and are plain *client* elements —
    /// they must NOT sit inside a `window_control_area(Drag)` region, or the OS
    /// would treat them as caption and swallow the clicks.
    fn window_controls(&self, palette: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let base = |glyph: &str| {
            div()
                .w(px(44.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(rgb(palette.text))
                .child(glyph.to_string())
        };
        div()
            .flex()
            .flex_row()
            .h_full()
            .child(
                base("—")
                    .hover(move |s| s.bg(rgb(palette.tab_inactive)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, _| window.minimize_window())),
            )
            .child(
                base("▢")
                    .hover(move |s| s.bg(rgb(palette.tab_inactive)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, _| this.toggle_maximize(window)),
                    ),
            )
            .child(
                base("✕")
                    .hover(|s| s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, _| window.remove_window())),
            )
    }

    /// Flip maximize/restore. Maximize uses gpui's `zoom_window` (which renders
    /// the frameless maximized window correctly); restore uses the Win32 shim
    /// (gpui can't restore on Windows).
    fn toggle_maximize(&mut self, window: &mut Window) {
        if self.maximized {
            crate::winutil::restore(window);
            self.maximized = false;
        } else {
            crate::winutil::maximize(window);
            self.maximized = true;
        }
        window.refresh();
    }

    /// A draggable region. Single-click + drag moves the window (drag starts
    /// only after the pointer passes a small threshold, so tiny jitter during a
    /// double-click doesn't trigger it); double-click toggles maximize/restore.
    fn drag_region(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, _| {
                    if ev.click_count >= 2 {
                        this.titlebar_press = None;
                        this.toggle_maximize(window);
                    } else {
                        this.titlebar_press = Some(ev.position);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, _| {
                if let Some(start) = this.titlebar_press {
                    let dx = f32::from(ev.position.x - start.x).abs();
                    let dy = f32::from(ev.position.y - start.y).abs();
                    if dx + dy > 4.0 {
                        this.titlebar_press = None;
                        // Dragging a maximized window restores it (Windows does
                        // this automatically for a caption drag); sync our flag.
                        this.maximized = false;
                        crate::winutil::start_drag(window);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.titlebar_press = None;
                }),
            )
    }

    fn titlebar(&self, palette: Palette, narrow: bool, cx: &mut Context<Self>) -> gpui::Div {
        let bar = || {
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .h(px(34.))
                .bg(rgb(palette.surface))
                .border_b_1()
                .border_color(rgb(palette.border))
                .font_family("Segoe UI")
                .text_size(px(13.))
        };

        if self.focus_mode {
            return bar()
                .child(
                    self.drag_region(cx)
                        .flex_1()
                        .min_w(px(0.))
                        .px_2()
                        .gap_2()
                        .child(self.logo(palette))
                        .child(div().text_color(rgb(palette.muted)).child("RustedNote — focus mode")),
                )
                .child(self.window_controls(palette, cx));
        }

        let vertical = cx.global::<Preferences>().tab_orientation == TabDir::Vertical;
        let view_label = if vertical { "V View" } else { "H View" };

        // Interactive nav (client elements — clickable).
        let nav = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_1()
            .child(self.tb_button("File ▾").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.show_file_menu = !this.show_file_menu;
                    this.show_recent_submenu = false;
                    if this.show_file_menu {
                        this.blur_editor(window, cx);
                    } else {
                        this.focus_active(window, cx);
                    }
                    cx.notify();
                }),
            ))
            .child(self.tb_button(view_label).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.toggle_tabs(&ToggleTabs, window, cx)),
            ))
            .child(self.tb_button("⚙").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.toggle_settings(&ToggleSettings, window, cx)),
            ));

        // Big draggable filler between nav and the window controls.
        let filler = self.drag_region(cx).flex_1().min_w(px(0.)).px_2();

        bar()
            // Logo + app name sit in their own drag region on the far left. On
            // a narrow window the name drops so the nav buttons keep their room.
            .child(
                self.drag_region(cx)
                    .px_2()
                    .gap_2()
                    .child(self.logo(palette))
                    .when(!narrow, |d| {
                        d.child(
                            div()
                                .whitespace_nowrap()
                                .text_color(rgb(palette.text))
                                .child("RustedNote"),
                        )
                    }),
            )
            .child(nav)
            .child(filler)
            .child(self.window_controls(palette, cx))
    }

    fn render_file_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |label: SharedString| {
            div()
                .px_3()
                .py(px(2.))
                .rounded_sm()
                .cursor_pointer()
                .whitespace_nowrap()
                .text_color(rgb(palette.text))
                .hover(move |s| s.bg(rgb(palette.tab_inactive)))
                .child(label)
        };
        let separator = || div().my_1().h(px(1.)).w_full().bg(rgb(palette.border));

        let recent_row = div()
            .id("mi-recent")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py(px(2.))
            .rounded_sm()
            .cursor_pointer()
            .when(self.show_recent_submenu, |d| d.bg(rgb(palette.tab_inactive)))
            .hover(move |s| s.bg(rgb(palette.tab_inactive)))
            .child("Recent Files")
            .child("▸")
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering {
                    this.show_recent_submenu = true;
                    cx.notify();
                }
            }));

        let panel = div()
            .id("file-menu")
            .flex()
            .flex_col()
            .gap(px(1.))
            .p_2()
            .min_w(px(260.))
            .rounded_md()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            // Clicks inside the menu must not dismiss it.
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(item("Open…".into()).id("mi-open").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.open_file(&OpenFile, window, cx)),
            ))
            .child(item("Save".into()).id("mi-save").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_file_menu = false;
                    let ed = this.active_editor().clone();
                    ed.update(cx, |e, cx| e.perform_save(cx));
                    cx.notify();
                }),
            ))
            .child(item("Save As…".into()).id("mi-saveas").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_file_menu = false;
                    let ed = this.active_editor().clone();
                    ed.update(cx, |e, cx| e.perform_save_as(cx));
                    cx.notify();
                }),
            ))
            .child(separator())
            .child(recent_row)
            .child(separator())
            .child(item("Bookmark Current File".into()).id("mi-bmk").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.show_file_menu = false;
                    this.show_recent_submenu = false;
                    this.bookmark_current(&BookmarkCurrent, window, cx);
                }),
            ))
            .child(item("Bookmarks…".into()).id("mi-bmks").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.toggle_bookmarks(&ToggleBookmarks, window, cx)),
            ))
            .child(separator())
            .child(item("Font Family…".into()).id("mi-fonts").on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering && this.show_recent_submenu {
                    this.show_recent_submenu = false;
                    cx.notify();
                }
            })).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.open_fonts(window, cx)),
            ));

        // Full-window catcher that closes the menu on outside click, plus the
        // menu panel and (optionally) the Recent flyout beside it.
        let mut root = div()
            .track_focus(&self.modal_focus)
            .key_context("Modal")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.show_file_menu = false;
                    this.show_recent_submenu = false;
                    this.focus_active(window, cx);
                    cx.notify();
                }),
            )
            .child(div().absolute().top(px(38.)).left(px(8.)).child(panel));

        if self.show_recent_submenu {
            root = root.child(
                div()
                    .absolute()
                    .top(px(120.))
                    .left(px(276.))
                    .child(self.render_recent_flyout(palette, cx)),
            );
        }
        root
    }

    fn render_recent_flyout(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("recent-flyout")
            .flex()
            .flex_col()
            .gap(px(1.))
            .p_2()
            .min_w(px(240.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .rounded_md()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if *hovering {
                    this.show_recent_submenu = true;
                    cx.notify();
                }
            }));

        if self.recent.paths.is_empty() {
            return list.child(div().px_3().py(px(2.)).text_color(rgb(palette.muted)).child("(none)"));
        }

        for path in &self.recent.paths {
            let open_path = path.clone();
            let remove_path = path.clone();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("untitled")
                .to_string();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py(px(2.))
                    .rounded_sm()
                    .hover(move |s| s.bg(rgb(palette.tab_inactive)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .cursor_pointer()
                            .child(name)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.show_file_menu = false;
                                    this.show_recent_submenu = false;
                                    this.open_path(open_path.clone(), window, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(palette.muted))
                            .hover(move |s| s.text_color(rgb(palette.text)))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_recent(remove_path.clone(), cx);
                                }),
                            ),
                    ),
            );
        }

        list.child(div().my_1().h(px(1.)).w_full().bg(rgb(palette.border)))
            .child(
                div()
                    .px_3()
                    .py(px(2.))
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(palette.text))
                    .hover(move |s| s.bg(rgb(palette.tab_inactive)))
                    .child("Clear All Recents")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.show_file_menu = false;
                            this.show_recent_submenu = false;
                            this.clear_recent(cx);
                        }),
                    ),
            )
    }

    // --- rendering: tabs --------------------------------------------------

    fn tab_chip(
        &self,
        index: usize,
        palette: Palette,
        vertical: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (title, is_dirty) = {
            let ed = self.editors[index].read(cx);
            (ed.title(), ed.is_dirty())
        };
        let active = index == self.active;

        let mut chip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(1.))
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(if active {
                palette.tab_active
            } else {
                palette.tab_inactive
            }))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(if active { palette.accent } else { palette.tab_inactive }));

        chip = if vertical {
            chip.w_full()
        } else {
            chip.max_w(px(200.))
        };

        chip.child(div().flex_1().min_w(px(0.)).truncate().child(title))
            .when(is_dirty, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(palette.accent))
                        .child("●"),
                )
            })
            .child(
                div()
                    .px_1()
                    .rounded_sm()
                    .flex_shrink_0()
                    .text_color(rgb(palette.muted))
                    .hover(move |s| s.text_color(rgb(palette.text)))
                    .child("×")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_index(index, window, cx);
                        }),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.select_tab(index, window, cx)),
            )
    }

    fn layout_horizontal(&self, palette: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let mut strip = div()
            .id("tab-strip")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w(px(0.))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);
        for i in 0..self.editors.len() {
            strip = strip.child(self.tab_chip(i, palette, false, cx));
        }

        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py(px(2.))
            .bg(rgb(palette.surface))
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(strip)
            .child(self.tb_button("+").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)),
            ));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(bar)
            .child(div().flex_1().min_h(px(0.)).child(self.active_editor().clone()))
    }

    fn layout_vertical(&self, palette: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div()
            .id("tab-strip-v")
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.tab_scroll);
        for i in 0..self.editors.len() {
            list = list.child(self.tab_chip(i, palette, true, cx));
        }

        let sidebar = div()
            .flex()
            .flex_col()
            .w(px(220.))
            .h_full()
            .p_2()
            .gap_2()
            .bg(rgb(palette.surface))
            .border_r_1()
            .border_color(rgb(palette.border))
            .child(self.tb_button("+ New Tab").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)),
            ))
            .child(list);

        div()
            .flex()
            .flex_row()
            .size_full()
            .child(sidebar)
            .child(div().flex_1().min_w(px(0.)).child(self.active_editor().clone()))
    }

    // --- rendering: overlays ---------------------------------------------

    fn render_settings(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = cx.global::<Preferences>().clone();

        let themes = [
            ThemeChoice::System,
            ThemeChoice::Light,
            ThemeChoice::DarkSoft,
            ThemeChoice::SepiaWarm,
            ThemeChoice::HighContrast,
        ];
        let mut theme_row = div().flex().flex_row().flex_wrap().gap_2();
        for choice in themes {
            let active = choice == prefs.theme;
            theme_row = theme_row.child(
                div()
                    .px_3()
                    .py(px(6.))
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if active { palette.accent } else { BTN_BG }))
                    .text_color(rgb(if active { palette.background } else { BTN_TEXT }))
                    .child(choice.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_theme(choice, cx)),
                    ),
            );
        }

        let step_button = |label: &str| {
            div()
                .w(px(32.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(BTN_BG))
                .text_color(rgb(BTN_TEXT))
                .child(label.to_string())
        };

        let font_size_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(90.)).child("Font size"))
            .child(step_button("–").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.font_smaller(cx)),
            ))
            .child(div().w(px(56.)).text_center().child(format!("{} pt", prefs.font_size as u32)))
            .child(step_button("+").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.font_bigger(cx)),
            ));

        let zoom_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(90.)).child("Zoom"))
            .child(step_button("–").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.apply_zoom_out(cx)),
            ))
            .child(div().w(px(56.)).text_center().child(format!("{}%", prefs.zoom_percent())))
            .child(step_button("+").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.apply_zoom_in(cx)),
            ));

        // Simple ON/OFF toggle pill, mirroring the theme-choice pill style.
        let toggle_pill = |active: bool, on_label: &str, off_label: &str| {
            div()
                .px_3()
                .py(px(4.))
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(if active { palette.accent } else { BTN_BG }))
                .text_color(rgb(if active { palette.background } else { BTN_TEXT }))
                .child(if active { on_label.to_string() } else { off_label.to_string() })
        };

        let word_wrap_now = prefs.word_wrap;
        let word_wrap_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(180.)).child("Word wrap"))
            .child(
                toggle_pill(word_wrap_now, "On", "Off").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.set_word_wrap(!word_wrap_now, cx)),
                ),
            );

        let line_numbers_now = prefs.show_line_numbers;
        let line_numbers_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(180.)).child("Show line numbers"))
            .child(
                toggle_pill(line_numbers_now, "On", "Off").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_show_line_numbers(!line_numbers_now, cx)
                    }),
                ),
            );

        let restore_now = prefs.restore_session;
        let restore_session_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(180.)).child("Open where you left off"))
            .child(
                toggle_pill(restore_now, "On", "Off").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.set_restore_session(!restore_now, cx)),
                ),
            );

        // Keyboard shortcuts reference — laid out inline, since the panel body
        // is now the single scroll region (a nested one would fight with it).
        let mut shortcut_list = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(1.))
            .py_1()
            .rounded_md()
            .bg(rgb(palette.background));
        for (action, keys) in SHORTCUTS {
            shortcut_list = shortcut_list.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap_4()
                    .px_2()
                    .py(px(2.))
                    .child(div().child(*action))
                    .child(div().text_color(rgb(palette.muted)).whitespace_nowrap().child(*keys)),
            );
        }

        // Everything below the header scrolls as one region, so no setting can
        // end up unreachable on a short window.
        let body = div()
            .id("settings-body")
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(rgb(palette.muted)).child("Theme"))
            .child(theme_row)
            .child(font_size_row)
            .child(zoom_row)
            .child(div().text_color(rgb(palette.muted)).child("Editor"))
            .child(word_wrap_row)
            .child(line_numbers_row)
            .child(restore_session_row)
            .child(div().text_color(rgb(palette.muted)).child("System"))
            .child(self.open_with_row(palette, cx))
            .child(div().text_color(rgb(palette.muted)).child("Keyboard shortcuts"))
            .child(shortcut_list);

        let panel = div()
            .w_full()
            .max_w(px(540.))
            .max_h(px(self.vp_height - 24.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .rounded_lg()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            // Header stays put; the body below it scrolls.
            .child(self.panel_header("Settings", cx))
            .child(body);

        self.overlay(panel)
    }

    /// Register / remove RustedNote as a plain-text handler, so right-clicking
    /// a .txt in the file manager offers it. Per-user and fully reversible.
    fn open_with_row(&self, palette: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let registered = integration::is_registered();

        let mut row = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .flex_1()
                            // A flex item won't shrink below its content unless
                            // told to; without this the description pushes the
                            // button out of the panel.
                            .min_w(px(0.))
                            .child(if registered {
                                "Open text files with RustedNote — registered"
                            } else {
                                "Open text files with RustedNote"
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.muted))
                                    .child(integration::describe()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_3()
                            .py(px(4.))
                            .rounded_md()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .bg(rgb(if registered { BTN_BG } else { palette.accent }))
                            .text_color(rgb(if registered {
                                BTN_TEXT
                            } else {
                                palette.background
                            }))
                            .child(if registered { "Remove" } else { "Register" })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_open_with(!registered, cx)
                                }),
                            ),
                    ),
            );

        if let Some(status) = &self.open_with_status {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.accent))
                    .child(status.clone()),
            );
        }
        row
    }

    /// Dedicated Font Family picker modal (opened from the File menu). The panel
    /// itself does not scroll; the font list (flex-fill) is the single scroll
    /// region, so it can be tall without the scroll-in-scroll rendering bug.
    fn render_fonts(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let current = cx.global::<Preferences>().font_family.clone();

        let list_h = (self.vp_height - 140.).max(220.);
        let mut list = div()
            .id("fonts-list")
            .w_full()
            .h(px(list_h))
            .overflow_y_scroll()
            .track_scroll(&self.font_scroll)
            .flex()
            .flex_col()
            .rounded_md()
            .bg(rgb(palette.background));
        for name in &self.fonts {
            let n = name.clone();
            let active = n.as_ref() == current.as_str();
            let hov = palette.tab_inactive;
            list = list.child(
                div()
                    .px_3()
                    .py(px(4.))
                    .w_full()
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .text_color(rgb(if active { palette.background } else { palette.text }))
                    .when(active, |d| d.bg(rgb(palette.accent)))
                    .hover(move |s| s.bg(rgb(hov)))
                    .child(n.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_font(n.to_string(), cx)),
                    ),
            );
        }

        let panel = div()
            .w_full()
            .max_w(px(460.))
            .max_h(px(self.vp_height - 24.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .rounded_lg()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            .child(self.panel_header("Font Family", cx))
            .child(list);

        self.overlay(panel)
    }

    fn render_bookmarks(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full();

        if self.bookmarks.items.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_color(rgb(palette.muted))
                    .child("No bookmarks yet. Press Ctrl+D to bookmark the current file."),
            );
        }

        for bm in &self.bookmarks.items {
            let path = bm.path.clone();
            let path_for_remove = bm.path.clone();
            let missing = !bm.exists();
            let subtitle = bm.path.to_string_lossy().to_string();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(palette.background))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .cursor_pointer()
                            .child(
                                div()
                                    .truncate()
                                    .text_color(rgb(if missing { palette.muted } else { palette.text }))
                                    .child(if missing {
                                        format!("{} (missing)", bm.name)
                                    } else {
                                        bm.name.clone()
                                    }),
                            )
                            .child(div().truncate().text_color(rgb(palette.muted)).child(subtitle))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.open_bookmark(path.clone(), window, cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(palette.muted))
                            .hover(move |s| s.text_color(rgb(palette.text)))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_bookmark(path_for_remove.clone(), cx)
                                }),
                            ),
                    ),
            );
        }

        let panel = div()
            .id("bookmarks-panel")
            .w_full()
            .max_w(px(560.))
            .max_h(px(self.vp_height - 24.))
            .overflow_y_scroll()
            .track_scroll(&self.bmk_panel_scroll)
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_lg()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            .child(self.panel_header("Bookmarks", cx))
            .child(list);

        self.overlay(panel)
    }

    /// "Save before closing?" — shown only for an untitled note with content,
    /// where closing really would lose the only copy.
    fn render_close_confirm(
        &self,
        index: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = self
            .editors
            .get(index)
            .map(|editor| editor.read(cx).title())
            .unwrap_or_else(|| "Untitled".to_string());

        self.overlay(
            div()
                .w(px(440.))
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .rounded_lg()
                .bg(rgb(palette.surface))
                .border_1()
                .border_color(rgb(palette.border))
                .text_color(rgb(palette.text))
                .child(
                    div()
                        .text_size(px(15.))
                        .child(format!("Save changes to {title}?")),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(palette.muted))
                        .child(
                            "This note has never been saved to a file. If you close it \
                             without saving, what you have written is gone.",
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .pt_1()
                        .child(self.button("Save…").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.confirm_close_save(window, cx)),
                        ))
                        .child(self.button("Don't Save").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.confirm_close_discard(window, cx)
                            }),
                        ))
                        .child(self.button("Cancel").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.confirm_close_cancel(window, cx)),
                        )),
                ),
        )
    }

    fn render_find_bar(&self, palette: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let find_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w(px(0.)).child(self.find_field.clone()))
            .child(self.button("↑").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.find_prev(&FindPrev, window, cx)),
            ))
            .child(self.button("↓").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.find_next(&FindNext, window, cx)),
            ))
            .child(self.button("×").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_settings(&CloseSettings, window, cx)),
            ));

        let mut bar = div()
            .key_context("Find")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(palette.surface))
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(find_row);

        if self.replace_mode {
            bar = bar.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w(px(0.)).child(self.replace_field.clone()))
                    .child(self.button("Replace").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.replace_one(&ReplaceOne, window, cx)),
                    ))
                    .child(self.button("All").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.replace_all(&ReplaceAll, window, cx)),
                    )),
            );
        }
        bar
    }

    fn render_palette(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let cmds = self.filtered_commands(cx);
        let mut list = div()
            .id("palette-list")
            .flex()
            .flex_col()
            .gap(px(1.))
            .max_h(self.list_height(0.5, 140., 320.))
            .overflow_y_scroll()
            .track_scroll(&self.palette_scroll);
        for (i, (label, id)) in cmds.iter().enumerate() {
            let selected = i == self.palette_selected;
            let id = *id;
            let hov = palette.tab_inactive;
            list = list.child(
                div()
                    .px_3()
                    .py(px(2.))
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(palette.text))
                    .when(selected, |d| d.bg(rgb(palette.selection)))
                    .hover(move |s| s.bg(rgb(hov)))
                    .child(*label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.show_palette = false;
                            this.run_command(id, window, cx);
                        }),
                    ),
            );
        }

        let panel = div()
            .key_context("Palette")
            .w_full()
            .max_w(px(560.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .border_1()
            .border_color(rgb(palette.border))
            .child(self.palette_field.clone())
            .child(list);

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .px_4()
            .cursor(CursorStyle::Arrow)
            .bg(rgba(0x00000088))
            .child(div().mt_20().child(panel))
    }

    fn panel_header(&self, title: &str, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .child(div().text_xl().child(title.to_string()))
            .child(
                div()
                    .w(px(30.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(BTN_TEXT))
                    .bg(rgb(BTN_BG))
                    .hover(|s| s.bg(rgb(0xc42b1c)))
                    .child("✕")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_settings(&CloseSettings, window, cx)
                        }),
                    ),
            )
    }

    fn overlay(&self, panel: impl IntoElement) -> gpui::Div {
        div()
            .track_focus(&self.modal_focus)
            .key_context("Modal")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // Swallow mouse events — including the wheel — so scrolling over a
            // modal doesn't also scroll the editor behind it.
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .cursor(CursorStyle::Arrow)
            .bg(rgba(0x00000088))
            .child(panel)
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Track viewport size for responsive layout (overlays, titlebar).
        let vp = window.viewport_size();
        self.vp_width = f32::from(vp.width);
        self.vp_height = f32::from(vp.height);
        let narrow = self.vp_width < 760.;

        let theme = cx.global::<Preferences>().theme;
        let palette = Palette::for_choice(theme.resolve(os_is_dark(window)));
        let vertical = cx.global::<Preferences>().tab_orientation == TabDir::Vertical;

        // Custom titlebar is always present (it hosts the window controls);
        // in focus mode it collapses to logo + drag + controls.
        let mut col = div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.titlebar(palette, narrow, cx));
        let body = if self.focus_mode {
            col.child(div().flex_1().min_h(px(0.)).child(self.active_editor().clone()))
        } else {
            if self.show_find {
                col = col.child(self.render_find_bar(palette, cx));
            }
            let area = if vertical {
                self.layout_vertical(palette, cx)
            } else {
                self.layout_horizontal(palette, cx)
            };
            col.child(div().flex_1().min_h(px(0.)).child(area))
        };

        let mut root = div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(rgb(palette.background))
            .text_color(rgb(palette.text))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::close_settings))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::toggle_tabs))
            .on_action(cx.listener(Self::toggle_bookmarks))
            .on_action(cx.listener(Self::bookmark_current))
            .on_action(cx.listener(Self::cycle_theme))
            .on_action(cx.listener(Self::toggle_focus))
            .on_action(cx.listener(Self::show_find))
            .on_action(cx.listener(Self::show_replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::replace_one))
            .on_action(cx.listener(Self::replace_all))
            .on_action(cx.listener(Self::show_palette))
            .on_action(cx.listener(Self::palette_up))
            .on_action(cx.listener(Self::palette_down))
            .on_action(cx.listener(Self::palette_confirm))
            .child(body);

        if self.show_file_menu && !self.focus_mode {
            root = root.child(self.render_file_menu(palette, cx));
        }
        if self.show_settings {
            root = root.child(self.render_settings(palette, cx));
        }
        if self.show_fonts {
            root = root.child(self.render_fonts(palette, cx));
        }
        if self.show_bookmarks {
            root = root.child(self.render_bookmarks(palette, cx));
        }
        if self.show_palette {
            root = root.child(self.render_palette(palette, cx));
        }
        if let Some(index) = self.confirm_close {
            root = root.child(self.render_close_confirm(index, palette, cx));
        }
        root
    }
}
