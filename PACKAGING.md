# Packaging & distribution — RustedNote

The crate is `rustednote`; the shipped binary is branded **`RustedNote.exe`**
(via `[[bin]] name = "RustedNote"` in `Cargo.toml`). On Windows the executable
carries an embedded **icon** and **version metadata** (Product name, File
description, Company), added by `build.rs` using the `winresource` crate and
`assets/icon.ico`.

## Build a release binary

```sh
cargo build --release
```

Output: `target/release/RustedNote.exe` (optimized, LTO on — see
`[profile.release]`). Double-click it or run it from a terminal.

## Distributing the .exe (Windows)

`RustedNote.exe` can be built fully self-contained (portable) — GPUI statically
links its GPU backend, and the MSVC C runtime can be linked statically too, so
the exe does **not** need `VCRUNTIME140.dll` / the Visual C++ Redistributable.
`.cargo/` is git-ignored, so create `.cargo/config.toml` locally before a
release build:

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Without it the build still works, but the exe needs the Visual C++
Redistributable (x64) on the target machine. With it, every DLL it imports ships
with Windows 10/11, and the only requirement for target machines is:

- A GPU/driver supporting **DirectX 11+** (any modern Windows machine).

For a named portable copy:

```sh
cargo build --release
copy target\release\RustedNote.exe target\portable\RustedNote-0.4.6-portable-x64.exe
```

Just copy `RustedNote.exe` to the target machine and run it. No admin install
required.

## MSI installer (`cargo wix`)

An MSI is fully configured and ready to build. `cargo-wix` is installed and a
**branded WiX source** lives at `wix/main.wxs`:

- Product **RustedNote**, installs to `C:\Program Files\RustedNote\bin\`
- **Start-menu shortcut** to `RustedNote.exe` (uses `assets/icon.ico`)
- Add/Remove-Programs icon, and adds the install dir to `PATH`
- Declares RustedNote as an app that can open text files — see
  ["Open with" registration](#open-with-registration) for what it does and does
  not touch

Installing a newer MSI upgrades an existing install in place: the `UpgradeCode`
is fixed and `MajorUpgrade` is scheduled `afterInstallInitialize`, so the old
version is removed first. **Never change the `UpgradeCode`** — that would make
the two versions install side by side. Bump `version` in `Cargo.toml`; the MSI
picks it up automatically via `$(var.Version)`.

### Prerequisite: WiX Toolset v3 (installed)

WiX Toolset v3.14 is installed at `C:\Program Files (x86)\WiX Toolset v3.14`.
If `candle.exe` isn't on `PATH`, pass the bin dir explicitly (as below).

### Build the MSI

From the project root:

```sh
cargo build --release
cargo wix --bin-path "C:\Program Files (x86)\WiX Toolset v3.14\bin" \
          -o target/wix/RustedNote-0.4.6-x86_64.msi
```

Output: `target/wix/RustedNote-0.4.6-x86_64.msi` (~3.3 MB). Double-click to
install — Program Files install + Start-menu shortcut; uninstall via
Add/Remove Programs. Editing `wix/main.wxs` customizes the installer further.

> Known benign warnings: two `LGHT1076/ICE69` (the Start-menu shortcut and the
> `shell\open\command` value both reference `exe0`, which lives in a sibling
> component within the same feature) — standard pattern, safe to ignore.

### Alternative: cross-platform bundles

`cargo install cargo-bundle` → `cargo bundle --release` produces `.msi`/`.app`/
`.deb`. Not required — the bare `RustedNote.exe` runs on its own.

## User data locations

RustedNote stores everything under the per-user config dir (keyed on the product
name `RustedNote`, not the crate name), so it never writes next to the
executable:

- Windows: `%APPDATA%\RustedNote\config\`
  - `preferences.json` · `bookmarks.json` · `recent.json` · `session.json`

Deleting that folder resets the app to defaults. Builds before this rename wrote
to `%APPDATA%\notepad_clone\config\`; that folder is stale and safe to delete.

## Single instance (Windows)

Opening a second file while RustedNote is already running — e.g. double-
clicking another `.txt` in Explorer — opens it as a **new tab in the existing
window**, rather than spawning a second process with a second window. This
matters more than it sounds: before this existed, closing the "wrong" window
(or just the original) could silently lose whatever was still unsaved in an
Untitled tab there, since nothing tied the two windows or their content
together.

Implementation is `src/single_instance.rs`: a named pipe
(`\\.\pipe\RustedNote.SingleInstance`) created with
`FILE_FLAG_FIRST_PIPE_INSTANCE`, so creating it doubles as the single-instance
check — only the very first launch can succeed, atomically, with no separate
mutex and no race between two instances started at once. That first launch
becomes the *primary*: a background thread keeps accepting connections for the
rest of the process's life. Every later launch fails to create the pipe,
becomes a client instead, writes its file path into it (or nothing, if it has
none — a plain relaunch with no file still brings the window to the front),
and calls `std::process::exit` immediately, **before any part of gpui is
initialized** — no window ever flashes on screen for it. The primary polls for
incoming paths on a 200ms timer and, for each one, opens it as a new tab and
calls `window.activate_window()` to bring itself to the foreground.

Not yet implemented on macOS/Linux — every launch there still opens its own
window, same as before this fix.

## "Open with" registration

Responsibility is split in two, deliberately.

**The MSI** only *declares* the app, under
`HKLM\SOFTWARE\Classes\Applications\RustedNote.exe`: a `FriendlyAppName`, an
open command, and a `SupportedTypes` list mirroring `EXTENSIONS` in
`src/integration.rs`. That is what makes "Choose another app" show *RustedNote*
rather than a bare exe path. It writes nothing into any extension's
`OpenWithList`, claims no default, and every key is component-tracked so
uninstall removes it.

**The app** does the part that changes what you see in the shell. Settings →
**System** offers a Register / Remove toggle that adds RustedNote to the *Open
with* menu for `.txt`, `.log`, `.md`, `.json` and the other extensions in
`EXTENSIONS`. It is:

- **per-user** — `HKCU\Software\Classes` only, no admin rights, no machine-wide
  keys touched;
- **additive** — it lists the `RustedNote.TextDocument` ProgID under each
  extension's `OpenWithProgids`, so the user's default editor is untouched;
- **fully reversible** — Remove deletes exactly the ProgID key and the values it
  added, leaving other apps' entries alone.

The button reads as "Register" again if the exe moves (e.g. after switching from
a dev build to the installed copy), because a stale path counts as unregistered.
So after installing the MSI, open Settings → System once and hit Register.

### Cleanup on uninstall

Registration is written by the *app*, at runtime, into `HKCU`. The installer
never saw those keys, so it cannot component-track them — left alone they would
outlive the uninstall, and the shell would keep offering "RustedNote" for `.txt`
while launching an executable that is no longer there.

Three mechanisms close that gap:

1. **A headless switch.** `RustedNote.exe --unregister-shell` performs exactly
   what Settings → System → Remove does, then exits without opening a window.
   (`--register-shell` is the mirror image, for scripted setup.) Both are
   handled before GPUI starts, so they are safe to call from an installer.
2. **A WiX custom action.** `UnregisterShellIntegration` in `wix/main.wxs` runs
   that switch during uninstall. Two scheduling details carry the whole thing:
   - `Execute='immediate'` puts it in the msiexec **client** process, which runs
     as the logged-on user, so `HKCU` is *that user's* hive. A deferred action
     runs in the elevated server process, where `HKCU` can resolve to the
     SYSTEM profile and clean nothing.
   - `Before='RemoveFiles'` is the last moment `RustedNote.exe` still exists to
     be run at all.

   Its condition is `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE`. The second half
   matters: a major upgrade runs the *old* package's uninstall via
   `RemoveExistingProducts`, and without the guard, upgrading would silently
   unregister the user even though the new build lands at the same path.
   `Return='ignore'` means a failure here can never wedge an uninstall.
3. **Startup self-repair.** On launch the app calls `integration::prune_stale()`,
   which drops the registration if the executable it points at no longer exists.
   This is what cleans up after an *older* build that was uninstalled before any
   of the above existed. It only ever prunes a **dead** path — a registration
   pointing at another RustedNote that is still installed (a dev build, a second
   copy) is someone's working setup and is left untouched.

**Known limitation — other users.** An uninstall runs once, as one user, and can
only reach that user's `HKCU`. If several Windows accounts each hit Register,
the others keep their entry until they next launch a RustedNote (self-repair,
above) or clear it by hand:

```
reg delete "HKCU\Software\Classes\RustedNote.TextDocument" /f
for %E in (txt log md markdown ini cfg conf csv json xml yml yaml toml) do @reg delete "HKCU\Software\Classes\.%E\OpenWithProgids" /v RustedNote.TextDocument /f
```

That removes only RustedNote's own ProgID and its value under each extension;
every other application's `OpenWithProgids` entry is left alone.

## Cross-platform

`cargo build --release` also builds on macOS (Metal) and Linux (Blade/Vulkan);
GPUI is most mature on macOS, then Linux, then Windows. The custom titlebar uses
`appears_transparent` + `window_control_area`, which behaves per-platform. On
Linux you may also want `window_decorations` set; add it in `main.rs`
`WindowOptions` if targeting Linux.
