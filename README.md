# Orcaa Desktop

Tauri 2.x desktop wrapper for the [Orcaa](https://orcaa.cloud) PWA. One Rust
source tree, one app, builds for Windows + macOS + Linux.

| App     | productName | Identifier                     | Initial URL (fresh install) |
| ------- | ----------- | ------------------------------ | --------------------------- |
| Orcaa   | `Orcaa`     | `cloud.orcaa.business.desktop` | `https://auth.orcaa.cloud`  |

**One app serves every audience.** There used to be a second "Orcaa Admin" build
pointed at `admin.orcaa.cloud`; it compiled from this same crate and rendered the
same window, so it bought nothing but a second icon, release channel and updater
manifest. A platform admin now signs in through this app like anyone else — auth
hands them back under the reserved `admin` subdomain label and the window lands on
`admin.orcaa.cloud`. Anything that used to key off the build variant (the tray's
primary quick-jump) now reads the origin currently on screen instead.

The wrapper loads the live hosted PWA — no SPA bundling. App updates ship via the web deploy; only new native features require a desktop rebuild.

**Platform status:**

| Platform | Status                                                                                        | Installer formats           |
| -------- | --------------------------------------------------------------------------------------------- | --------------------------- |
| Windows 10 1809+ / 11 (x64) | ✅ CI release flow (`windows-latest`)                                       | `.msi`, `.exe` (NSIS)       |
| **Windows 7 SP1** (x64 + x86) | ✅ CI release flow — separate legacy lane, see "Windows 7 build" below     | `*-win7-setup.exe` (NSIS, WebView2 109 bundled) |
| macOS    | ✅ CI release flow (`macos-latest`, universal Intel + Apple Silicon) — no Mac hardware needed | `.dmg`                      |
| Linux    | ✅ CI release flow (`ubuntu-22.04`, x86_64)                                                   | `.deb`, `.rpm`, `.AppImage` |

All of them build from the same tag push — one workflow, one GitHub release.

### System requirements

- **Windows 10 (1809+) / 11, 64-bit** — the standard build (`orcaa-desktop.exe`). Needs the WebView2
  Evergreen runtime, preinstalled on these versions; the installer fetches it if missing.
- **Windows 7 SP1, 64-bit or 32-bit** — the legacy build (`orcaa-desktop-win7.exe` /
  `orcaa-desktop-win7-x86.exe`). Self-contained: it carries WebView2 **109.0.1518.140**, the last runtime
  Microsoft shipped for Windows 7 (Microsoft's own standalone installer, run by the setup only when no
  runtime is present), so it installs offline and needs no TLS 1.2 fix-ups. The one OS
  prerequisite is Windows Update applied through 2013: Microsoft's own WebView2 loader imports
  `EventSetInformation`, which Windows 7 SP1 gained with **KB2882822** (the import audit prints this as a
  warning on every build). Trade-offs, stated
  plainly to users on the downloads page: Chromium 109 receives **no further security updates**, the
  installer is ~150 MB larger, and Windows 7 has no toast centre — notifications show inside Orcaa and flash
  the taskbar button instead. Everything else (printing, cash drawer, badge, kiosk, deep links, updater) is
  identical.
- **macOS 10.15+** (universal), **Linux** x86_64 with glibc 2.35+ (Ubuntu 22.04+).

---

## Relationship to the PWA repo

The PWA source lives in a separate repo (`orcaa-apps`). This repo only contains the desktop wrapper. They communicate at runtime:

- This wrapper loads `https://auth.orcaa.cloud` into a WebView2 window, and from there whatever tenant (or `admin.orcaa.cloud`) the signed-in session belongs to
- The PWA detects it's inside Tauri via `window.__TAURI_INTERNALS__` and conditionally hides the "Notifications Blocked" browser banner + routes notifications through the OS via `@tauri-apps/plugin-notification`
- No compile-time coupling. You can ship desktop releases independently of PWA releases

The PWA-side glue (the `isTauri()` helper, the native-notification bridge, the suppressed banner) lives in the `orcaa-apps` repo at:

- `shared/utils/isTauri.ts`
- `shared/services/notification/tauriNotification.ts`
- `shared/components/banners/PushNotificationPrompt.tsx` (conditional return)
- `shared/context/WebSocketContext.tsx` (toast bridge)

---

## How the multi-tenant flow works

Orcaa Business is multi-tenant — every customer lives on their own subdomain (e.g. `mygym.orcaa.cloud`). The wrapper handles this:

1. **First launch** → loads `https://auth.orcaa.cloud`. User logs in / registers / picks their domain. The PWA redirects them to their tenant subdomain.
2. **Subsequent launches** → loads the last URL the user was on (persisted in `orcaa-desktop.json` in the app data dir). If session is still valid, they land back on their tenant. If expired, the PWA redirects them to `auth.orcaa.cloud` automatically.
3. **Logout** → the web app navigates to `auth.orcaa.cloud`. That URL gets saved as the "last URL" so the next launch goes straight to the login screen.
4. **External links** (OAuth providers, Stripe, marketing pages — anything not `*.orcaa.cloud`) are opened in the user's default browser via `tauri-plugin-opener`. This avoids Google/Microsoft's webview-OAuth block and keeps password managers in the loop.

The internal-URL allowlist lives in [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs) — `is_internal_url()`. It allows `orcaa.cloud` + any `*.orcaa.cloud` + any `*.orcaa.test` (dev).

---

## Background mode (tray + close behavior)

Closing the window (`X` button) **does NOT quit the app** — it minimizes to the system tray. This keeps the WebSocket connection alive so notifications continue to arrive even when no window is visible (the Slack/Discord/Teams pattern).

| Action                                                       | Result                                                                           |
| ------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| Click `X` (close window)                                     | Window hides; tray icon stays; WebSocket stays connected                         |
| Left-click tray icon                                         | Re-opens / focuses the window                                                    |
| Right-click tray icon → "Open Orcaa"                         | Same as above                                                                    |
| Right-click tray icon → "Check for Updates…"                 | Manual update check, always reports back (found / up to date / failed)           |
| Right-click tray icon → "Quit"                               | Persists session, then truly exits                                               |
| Launching the app a second time while one is already running | Focuses the existing window (single-instance via `tauri-plugin-single-instance`) |

The **first** time the window is closed, a one-off native toast explains that the app is still running in the tray — hiding on X is the right default, but it is surprising once. The flag lives in the store, so it never fires twice.

Memory cost: ~150–200 MB (WebView2 + Rust shell). Negligible CPU when idle.

---

## Sign-in happens in the browser, not in the app

The webview never renders a login page. Google returns `disallowed_useragent` for OAuth inside embedded
webviews, password managers can't reach into WebView2, and an app that draws its own login box is asking
people to type credentials into a window they can't verify. So the shell hands sign-in to the real browser
and waits for the result on an `orcaa://` deep link.

```
  shell                         system browser                 backend
    │  verifier (never leaves)
    │  challenge = sha256 ──────►  auth.orcaa.cloud?desktop=1
    │                               │  password / Google / 2FA
    │                               ├──── POST /auth/desktop-handoff ────►
    │                               ◄──── single-use ticket ─────────────┤
    ◄─── orcaa://auth?token=…&state=…┘
    │
    └─ webview → <tenant>/desktop-handoff?token=…&verifier=…  ── exchange ──►
                                                             ◄── JWTs ──────┘
```

**Any locally-installed program can register `orcaa://`**, so the deep link is assumed readable by an
attacker. Two independent things defend it:

- **PKCE.** The shell keeps a random `verifier` and sends only its SHA-256 through the browser. The backend
  refuses to redeem a ticket without the verifier, so a captured link is inert.
- **`state` matching.** A deep link the shell didn't initiate is dropped, so nothing can push an
  attacker-chosen session into the window. The pending attempt is consumed on first use, so links can't
  be replayed.

The returned `subdomain` is validated as a single DNS label before it is used to build a URL — otherwise a
crafted value could steer the webview off the tenant domain.

**The browser only ever opens on a click.** The welcome page's button fires the `signin_start` command from
a `click` handler — it is not a link, and there is no anchor anywhere on the page. This used to be an
`?action=signin` URL that `on_navigation` watched for, which meant *any* navigation producing that URL
opened a browser window; landing on the page after a sign-out or a cold launch could hijack the browser
with nobody having pressed anything. An `invoke` cannot be reached by navigation, so the class of bug is
gone rather than patched.

For the same reason, the "hand this off to the system browser" rule for external links is refused until the
app itself is on screen (i.e. the webview is sitting on an orcaa host). A redirect chain during boot must
never be able to launch a browser on its own; suppressed attempts are logged with the URL.

Where the parts live:

| Piece                                           | Location                                                           |
| ----------------------------------------------- | ------------------------------------------------------------------ |
| State/verifier, callback validation, unit tests | [`src-tauri/src/signin.rs`](src-tauri/src/signin.rs)               |
| Scheme registration                             | `plugins.deep-link.desktop.schemes` in `tauri.conf.json`           |
| Ticket issue + redeem                           | `DesktopHandoffService` / `DesktopHandoffController` (backend)     |
| Browser-side hand-back                          | `apps/auth/src/utils/desktopHandoff.ts` (orcaa-apps)               |
| In-app landing                                  | `apps/business/src/app/auth/desktop-handoff/page.tsx` (orcaa-apps) |

**Windows/Linux note:** an `orcaa://` link launches a _new_ process with the URL in argv rather than
signalling the running one. `tauri-plugin-single-instance` is therefore built with its `deep-link` feature
so the argument is forwarded into the live instance — without that feature the link silently opens a second
window instead of resuming sign-in.

**Platform admins take the same path.** The admin app ships a `/login` shim that forwards to
auth, and `completeHandoff` hands admins back under the reserved `admin` subdomain label — which the
shell resolves to `admin.orcaa.cloud/desktop-handoff`, a route the admin app serves. No second build,
no second scheme.

**The scheme is declared in `tauri.business.conf.json`, never in the shared base config.** Only one
desktop app ships, so only `orcaa` is ever registered. If a second build is ever added, give it a
*distinct* scheme: Windows hands a scheme to whichever installer ran last, so two apps sharing one
means a callback can be delivered to the app with no pending sign-in, which silently drops it and
leaves the other waiting on the holding page forever.

---

## What the Rust shell actually does

Wiring lives in [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs); the pages the shell draws itself are in
[`src-tauri/src/shell_page.rs`](src-tauri/src/shell_page.rs) and the update flow is in
[`src-tauri/src/updater.rs`](src-tauri/src/updater.rs).

| Concern                | Behaviour                                                                                                                                                                                                                                                               |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Launch**             | The window is visible from the first frame, showing a branded boot page served from `orcaa-shell://`. That page probes the tenant host and hands off to it. It replaces a hidden-until-first-paint window with a 12 s timeout, which showed nothing at all — sometimes for the whole twelve seconds — on a slow or dead network. |
| **Offline**            | When the probe fails the same document becomes a **"Can't reach Orcaa — Retry"** surface, so an unreachable backend never falls through to WebView2's own error page. It re-probes automatically on the browser `online` event.                                          |
| **Window geometry**    | Size / position / maximized / fullscreen restore across launches (`tauri-plugin-window-state`). Visibility is deliberately excluded from the flags — restoring "hidden" would make a launch appear to do nothing.                                                       |
| **Staying on screen**  | `clamp_window_to_work_area` pulls the window fully inside the _work area_ of whichever monitor its centre sits on, after restore and again on `ScaleFactorChanged`. **This is load-bearing:** the window-state plugin restores a saved position whenever the saved rect merely _intersects_ a monitor, so a window dragged until its title bar sat above the top edge came back exactly like that — caption buttons sliced in half, no title bar left to grab. |
| **Session continuity** | The current URL + geometry are saved on every exit path — close button, tray Quit, and the updater's pre-exit hook.                                                                                                                                                     |
| **External links**     | Any navigation off `*.orcaa.cloud` / `*.orcaa.test` is handed to the system browser — but only once the app itself is on screen, so a boot-time redirect can't open a browser on its own.                                                                               |
| **Keyboard**           | <kbd>Ctrl</kbd>+<kbd>±</kbd>/<kbd>0</kbd> and <kbd>Ctrl</kbd>+wheel are **swallowed** — the webview is pinned at 100% zoom (the UI is designed for it; the app's font-scale preference is the sanctioned knob), and `reset_zoom` re-asserts 1.0 after every navigation so a host-level zoom from before the lock can't survive an upgrade. <kbd>Ctrl</kbd>+<kbd>R</kbd>/<kbd>F5</kbd> refresh — which asks the app first via a cancelable `orcaa:refresh` event (`useDesktopRefreshBridge` re-pulls active queries, matching the app's "never reload, re-pull data" rule) and only reloads the webview if nothing cancels it; <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> always hard-reloads. <kbd>Alt</kbd>+<kbd>←</kbd>/<kbd>→</kbd> history (not mirrored for Arabic — Windows doesn't), <kbd>F11</kbd> fullscreen, <kbd>Ctrl</kbd>+<kbd>Q</kbd> quit. Bound by an injection script that claims **only** modified keys — the app owns bare keys, including its own <kbd>/</kbd> search. |
| **Context menu**       | **The app owns this** — `useGlobalContextMenu` already draws its own right-click menu and deliberately leaves the native one on editable fields for paste/spellcheck. The injected script must NOT bind `contextmenu`: it is injected at document-start, so calling `preventDefault()` makes the app's own handler see `defaultPrevented` and bail, leaving the page with **no** menu. The shell's own pages suppress it locally instead. Guarded by a test. |
| **Notifications**      | Toasts are drawn by the shell (`shell_notify`), not the notification plugin, so clicking one **raises the window and navigates** to the page it was about. An incoming voice call is drawn as `Scenario::IncomingCall` — pre-expanded, persistent, looping the system ringtone, with Answer / Decline. The `url` is validated against the app's hosts in Rust before anything is navigated. |
| **Audio**              | `--autoplay-policy=no-user-gesture-required`. Chromium drops audio no user gesture asked for, which is exactly the case for a notification sound arriving while the app sits in the tray — the toast would land in silence. Note this arg **replaces** wry's defaults, so they are repeated alongside it. |
| **Downloads**          | The download itself is left to WebView2; only the ending is ours — a toast naming the file, with a **Show in folder** button (`opener().reveal_item_in_dir`). Every export in the app is a blob plus a synthetic `<a download>`, so without this the user is left hunting in Downloads behind Edge's flyout. |
| **Start with Windows** | Opt-in, off by default, toggled from the tray (`tauri-plugin-autostart`, launched `--hidden` so a boot lands in the tray rather than throwing a window at whoever is signing in). The checkbox re-reads the real state after every toggle — a blocked registry write must not leave the menu claiming something untrue. |
| **File drag-and-drop** | `disable_drag_drop_handler()` — Tauri's handler is on by default and swallows HTML5 file drops before the page sees them, which silently broke every upload dropzone in the app. The shell listens for no drag events of its own.                                        |
| **Window title**       | Tracks the loaded page's `document.title`, falling back to the product name.                                                                                                                                                                                            |
| **Native strings**     | Tray, boot/offline pages, the update window and the tray hint are localized EN/AR from the OS display language ([`src-tauri/src/i18n.rs`](src-tauri/src/i18n.rs)). The tray is drawn before any page loads, so the webview's locale isn't available yet — the OS is the only signal that early. |
| **Diagnostics**        | Startup, update checks and failures are logged to the OS log dir via `tauri-plugin-log`.                                                                                                                                                                                |

### Commands and the app ACL

The crate exposes ~30 `#[tauri::command]`s, registered in `lib.rs` (`invoke_handler`) and grouped by
permission set in [`src-tauri/permissions/shell.toml`](src-tauri/permissions/shell.toml): sign-in
(`signin_start`), shell controls (`shell_reload`, `shell_fullscreen_toggle`, `shell_quit`), the custom
titlebar (`shell_window_control`, `shell_window_state`), the badge (`shell_badge`), notifications
(`shell_notify`), POS / label / kitchen / raster printing (`shell_pos_*`, `shell_label_*`,
`shell_kot_print`, `shell_kitchen_*`, `shell_print_raster`), the autostart offer and the update window
(`update_install`, `update_snooze`, `update_skip`).

⚠️ **`src-tauri/permissions/shell.toml` existing is what declares an app ACL manifest.** Tauri gates IPC
from a remote origin always, but from *local* pages only once that manifest exists. So the moment one
command is listed there, **every** command must be — including the ones the update window calls from a page
the shell itself served. A missing entry is a silent runtime rejection at the moment the button is pressed,
not a build error.

---

## Notifications

Three layers:

1. **Browser-permission banner suppression** — the PWA detects Tauri at runtime and skips its "Notifications Blocked" banner, which would be meaningless inside WebView2.
2. **In-app WebSocket events → native Windows toasts** — when a Reverb broadcast arrives and the window isn't focused, the PWA calls the shell's `shell_notify` command (a clickable toast that raises the window and navigates; `kind: "call"` rings). `@tauri-apps/plugin-notification` is only the fallback when that command isn't there.
3. **Background delivery** — covered by **(2) + the tray behavior above**. As long as the app is running (even minimized to tray), the WebSocket stays connected and toasts fire.

Push-when-truly-quit (after user picks Quit in tray) is a v2 concern — requires Windows Notification Service integration on the backend.

**Windows 7 build:** there is no toast centre and no `combase.dll`, so neither `tauri-winrt-notification`
nor `tauri-plugin-notification` is compiled in (see "Windows 7 build"). `shell_notify` instead flashes the
taskbar button (`FlashWindowEx`; an incoming call also brings a tray-hidden window back minimised so there
is a button to flash) and returns `false`. The PWA dispatches its in-app banner and chime *before* asking
the shell, so nothing is lost — and `isLegacyShell()` (`shared/utils/isTauri.ts`, reading the
`window.__ORCAA_SHELL__` global the injected script sets) stops it from falling through to the absent
plugin. A one-time in-app notice explains this to the user after their first sign-in on that build.

### ⚠️ The remote-origin capability (read before touching `capabilities/`)

Once signed in, the webview sits on an `https://*.orcaa.cloud` origin. **Tauri rejects every IPC call from
a remote origin** unless a capability lists that origin under `remote.urls`; `windows: ["main"]` alone only
covers the _local_ context. That grant lives in
[`src-tauri/capabilities/remote.json`](src-tauri/capabilities/remote.json), and it carries:
`notification:default`, `allow-shell-controls` (reload / fullscreen / quit, which the injected
keyboard script calls from the tenant page), `allow-shell-notify` (the clickable/ringing toast command
the PWA prefers over the plugin), `allow-shell-window` + `core:window:allow-start-dragging` +
`core:window:allow-internal-toggle-maximize` (the custom titlebar — see "Branded titlebar" below),
`allow-shell-badge` (the unread taskbar/dock badge), and a scope-restricted `opener:allow-open-url`
(https/http/mailto/tel only — deliberately not `open_path` or `reveal_item_in_dir`).

Delete or narrow it and `plugin:notification|notify` gets denied — the PWA's `sendNativeNotification()`
catches the rejection and returns `false`, so **the app looks completely healthy while no toast ever
fires**. The keyboard shortcuts fail the same silent way.

`default.json` stays `local`-only on purpose and covers the shell's own pages in the `main` and `updater`
windows. It is where `core:window:allow-start-dragging` lives — **not** part of `core:default`, and without
it the frameless update window's `data-tauri-drag-region` header cannot be dragged.

Note `https://*.orcaa.cloud` does **not** match the apex `https://orcaa.cloud` — that URL is listed
separately (URLPattern semantics).

**Windows 7 twins.** `capabilities-win7/default.json` and `capabilities-win7/remote.json` are copies of
the two files above minus `notification:default` — the notification plugin isn't registered in the Win7
build, and Tauri's ACL resolver parses every file under its pattern (before the enabled list is applied)
and rejects a permission from an absent plugin. So the Win7 build reads its own directory: `build.rs`
points tauri-build at `capabilities-win7/*.json` whenever the `legacy_win7` cfg is on. No
`app.security.capabilities` list is set anywhere on purpose — every file in the selected directory is
enabled, so each build is self-consistent without a config override (an explicit list naming `default`
would break the Win7 build, whose twin is `default-win7`). **Any permission you add to `default.json` / `remote.json` goes into the twin too**, unless it
comes from a plugin the Win7 build leaves out.

Windows toasts additionally need the installed app's Start Menu shortcut to carry
`System.AppUserModel.ID` = the bundle identifier. Tauri's NSIS installer does this automatically, which
is why toasts only work from an **installed** build — `pnpm dev:business` runs out of `target/debug`,
where the plugin deliberately skips setting the app ID.

---

## Branded titlebar (custom window chrome)

The desktop shell drops the stock OS window frame, and **the frontend web app owns and renders the topbar** with integrated window controls:
- **Frontend Topbar** — `AppTopbar` renders `<WindowControls />` (`shared/components/layout/WindowControls`) and sets `data-tauri-drag-region` on the topbar row.
- **Desktop IPC Bridge** — `shell_window_control` handles minimize, toggle-maximize, and close (which hides to tray). `shell_window_state` reports maximize and fullscreen status.

Per platform:

- **Windows** — frameless (`decorations(false)` + `shadow(true)`, which keeps resize borders and edge-snap). The frontend topbar draws caption buttons and acts as the window drag region.
- **macOS** — native traffic lights stay via `TitleBarStyle::Overlay` + `hidden_title`, with the web topbar applying left padding (`orcaa-desktop-mac`).
- **Linux** — native window frame with standard titlebar decorations.

## POS Station (silent printing, cash drawer, kiosk)

The desktop app's reason-to-install for retail counters — capabilities a browser tab can never have:

- **Silent ESC/POS receipt printing** (`shell_pos_print`) — the POS page sends a semantic op list
  (`text` / `pair` / `hr` / `qr` / `cut` / `drawer`); `src/print.rs` hand-encodes ESC/POS bytes and
  writes them straight to the printer over **TCP:9100** or a **serial COM port**. No driver, no
  spooler, no dialog. Payload capped at 400 ops; 4s wire timeout.
- **Cash drawer kick** — rides the receipt (`drawer` op) of a cash sale/refund only, and only when
  the saved config's `drawer_kick` is on. There is deliberately **no standalone kick command**:
  popping the till outside a sale is the classic skim window, so the capability does not exist.
- **Printer config** (`shell_pos_printer_get`/`set`) — interface, address, width (32/58mm ·
  48/80mm), `ESC t` codepage slot, text encoding (`cp1256` Arabic / `cp437` / `ascii`), drawer
  toggle. Persisted in the shell's store per machine — per STATION, which is exactly right for
  counter hardware. Validated on save so a broken config can't be persisted.
- **Test page** (`shell_pos_test_print`) — layout + an Arabic line + a QR in one print.
  **Arabic shaping is printer-firmware dependent** (many thermal printers shape/reorder cp1256
  on-printer; some don't) — the test page is how you find out for the hardware in front of you.
  Deliberately never kicks the drawer.
- **Kiosk / station mode** — launch with `--kiosk`: starts fullscreen (the titlebar strip retires
  itself), pairs with autostart so a counter PC boots straight into POS. Exit via tray Quit.

All five commands are grouped under the `allow-shell-pos` app permission (remote + local).

## Native presence

- **First-run autostart offer** — a **branded shell window** on first launch ("Start with your
  computer?"), wearing the same chrome as the update prompt (never the OS's stock message box);
  skipped when launched `--hidden` or already enabled; accepting flips the same autostart the tray
  checkbox controls (and syncs the checkbox through managed state). Asked exactly once. Window
  label `autostart` — listed in `default.json`'s `windows` with `allow-autostart-prompt`.
- **Unread badge** — `shell_badge(count)` mirrors the web bell's unread count onto the taskbar
  (Windows: red-dot overlay drawn in code) or dock (macOS/Linux: numeric badge). Wired from both
  AppTopbars via `useDesktopBadge`; zero (and sign-out) clears it.
- **Global summon shortcut** — Ctrl(⌘)+Shift+O toggles the window from anywhere, via
  `tauri-plugin-global-shortcut`. A registration conflict with another program is logged and
  ignored, never fatal.
- **Tray quick actions** — a primary jump + Dashboard. The primary jump follows the origin on
  screen: Point of Sale on a tenant, Today on `admin.*` (`sync_primary_nav_item` relabels it on
  every page load, so it never promises a page the current host doesn't serve). Both only swap the
  *path* on the origin the webview already sits on (`open_app_path`), so the tray can never steer
  the app to another host; before sign-in they simply raise the window.

---

## Local development

### Prereqs (one-time per machine)

**Windows:**

1. **Rust (MSVC)** — install via [rustup.rs](https://rustup.rs)
2. **Visual Studio 2022 Build Tools** — "Desktop development with C++" workload (~3 GB)
3. **WebView2 Runtime** — preinstalled on Win10 1809+ / Win11
4. **Node 20+ and pnpm 10+**
   (Building the **Windows 7** lane locally additionally needs a pinned nightly with `rust-src` — see
   "Windows 7 build" below.)
5. **Smart App Control: OFF** — Win11's SAC blocks unsigned build scripts; incompatible with Rust dev. (One-way switch — see Windows Security → App & browser control → Smart App Control settings.)

**macOS:**

1. **Xcode Command Line Tools** — `xcode-select --install`
2. **Rust** — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
3. **Node 20+ and pnpm 10+** — `brew install node@20 && npm install -g pnpm`

Then in this repo:

```bash
pnpm install
```

### Generate icons

```bash
pnpm icons:business
```

Reads the source PNG from `source-icons/business.png` and produces the full Tauri icon set (`32x32.png`, `128x128.png`, `icon.ico`, `icon.icns`, etc.) into `src-tauri/icons/business/`.

### Dev

```bash
pnpm dev:business   # opens window at https://auth.orcaa.cloud
```

DevTools: right-click → Inspect (debug builds only).

### Local release build

```bash
pnpm build:business
```

Output (Windows):

```
src-tauri/target/release/bundle/
  msi/   Orcaa_1.0.0_x64_en-US.msi
  nsis/  Orcaa_1.0.0_x64-setup.exe
         Orcaa_1.0.0_x64-setup.nsis.zip   (updater payload)
```

Output (macOS): `dmg/Orcaa_1.0.0_aarch64.dmg`, `macos/Orcaa.app(.tar.gz)`.

---

## Releasing via GitHub Actions (recommended)

A workflow at [`.github/workflows/release.yml`](.github/workflows/release.yml) builds and publishes a GitHub release on every tag push. No manual build required.

### One-time setup

**1. Configure repo secrets** (Settings → Secrets and variables → Actions → New repository secret):

| Secret name                          | Value                                                                                                                         |
| ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | Paste the **entire contents** of `~/.tauri/orcaa.key` (the private key file, including the `untrusted comment:` header line). |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The password you set when generating the key.                                                                                 |

Note: paste the **contents**, not the path. tauri-action expects the key as a string.

**2. Confirm `plugins.updater.pubkey`** in [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json) is set to the matching public key (just the base64 string from `orcaa.key.pub`).

### Cutting a release

> **The tag does not set the version.** The workflow reads it from the app config
> (`VERSION=$(jq -r '.version' "$CONF")`) and writes that into both the artifact
> filenames and `latest.json`. Tag `v1.0.10` on a config that still says `1.0.0`
> produces a release whose manifest advertises **1.0.0** — every installed client
> compares `1.0.0 > 1.0.0`, decides it is current, and never updates. This
> silently stalled updates for the whole 1.0.x line. Bump the config, always.

Before tagging, verify the macOS dependency graph even from a Windows workstation:

```bash
cargo fetch --locked --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin --target x86_64-apple-darwin
```

Release jobs also fetch each architecture's dependencies and build with `--locked`, so the
committed lockfile must resolve without CI changing versions. A Windows-only build can miss
dependencies used by the macOS updater.

Keep all four in lockstep — CI fails the release if any of them drift:

```bash
# 1. Bump every version field to the SAME number
#    src-tauri/tauri.conf.json          → "version": "1.0.3"
#    src-tauri/tauri.business.conf.json → "version": "1.0.3"
#    src-tauri/Cargo.toml               → version = "1.0.3"
#    package.json                       → "version": "1.0.3"

# 2. Add a "## 1.0.3" section to CHANGELOG.md — user-facing bullets only.
#    The manifest job puts that section verbatim into latest.json, and the
#    branded update window shows it to every user as "What's new". No entry
#    → CI warns and users get the generic "Improvements and fixes." line.

# 3. Commit + tag + push
git add -A
git commit -m "chore: bump to 1.0.3"
git tag v1.0.3
git push origin master --tags
```

Sanity-check the release afterwards — this catches the mismatch above in one call:

```bash
curl -sL https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/latest.json | jq .version
# must equal the version you just tagged
```

The workflow then:

1. Spins up `windows-latest`, `macos-latest` and `ubuntu-22.04` (serialized matrix), installs Rust + Node + pnpm, restores caches
2. Generates icons from `source-icons/business.png`
3. Runs `tauri build` per platform (macOS builds `--target universal-apple-darwin` — one binary for Intel + Apple Silicon)
4. Creates ONE GitHub release tagged `v1.0.1` that all three jobs upload into
5. Uploads `.exe` + `.msi` (Windows), `.dmg` + `.app.tar.gz` (macOS), `.deb` + `.rpm` + `.AppImage` (Linux) with their updater `.sig` files
6. Uploads stable-name copies for the landing site: `orcaa-desktop.exe`, `orcaa-desktop.dmg`, `orcaa-desktop.AppImage`
7. A final `manifest` job reads every `*.sig` on the release and composes ONE `latest.json` covering `windows-x86_64`, `darwin-aarch64`, `darwin-x86_64` and `linux-x86_64` — the auto-updater on every OS polls this single file

8. A `build-win7` job (x64, then x86) builds the Windows 7 lane described below, audits the exe's imports,
   and uploads `Orcaa_<v>_x64-win7-setup.exe` / `Orcaa_<v>_x86-win7-setup.exe` (+ `.sig`) plus the stable
   names `orcaa-desktop-win7.exe` / `orcaa-desktop-win7-x86.exe`
9. The `manifest` job runs twice: `latest.json` for everyone else, and `latest-win7.json`
   (`windows-x86_64` + `windows-i686`) that only the Win7 build polls — so a Windows 7 station is never
   offered an installer whose WebView2 it cannot run, and vice versa

Build time: ~5–10 min with caches, ~15–20 min cold. GitHub Actions is free for public repos and gives 2,000 Linux-minute equivalents/month for private (Windows costs 2×). The Win7 lane adds ~10 min per architecture (it compiles the standard library).

---

## Windows 7 build (legacy lane)

Windows 7 machines are still common at counters in Egypt and the Gulf, so a build that runs there is worth
its cost — but three facts about the platform force every choice below. Nothing here is optional.

| Fact | Consequence in this repo |
| ---- | ------------------------ |
| **Rust ≥ 1.78 binaries need Windows 10.** The only supported route is the tier-3 target `x86_64-win7-windows-msvc` / `i686-win7-windows-msvc`, which ships no prebuilt std. | The lane builds on a **pinned nightly** with `-Zbuild-std=std,panic_abort` (`CARGO_UNSTABLE_BUILD_STD` in the workflow; deliberately *not* in `.cargo/config.toml`, so stable builds stay untouched). Same crate, same `Cargo.lock`. |
| **Windows 7 has no `combase.dll` (WinRT).** An exe that statically imports it fails to *load*. | `Cargo.toml` selects `tauri-plugin-notification` / `tauri-winrt-notification` only for `cfg(not(target_vendor = "win7"))`; `build.rs` turns those triples into the `legacy_win7` cfg the source branches on; `.cargo/config.toml` adds `--cfg windows_slim_errors` (drops `windows-result`'s `RoOriginateErrorW`) and a static CRT. `cargo clippy --features win7` previews the same cfg on stable — CI runs it. |
| **`ctor` 0.8.0 (via tauri-utils) only knows Windows as `target_vendor = "pc"`** and hard-errors on the win7 triples, whose vendor is `win7`. | [`src-tauri/patches/ctor`](src-tauri/patches/ctor/PATCH-NOTES.md) is the crates.io 0.8.0 release with that one cfg widened, wired through `[patch.crates-io]`. It resolves to the same version, so every other platform compiles identical code. (ctor 1.0.x upstream already gates on `target_os = "windows"`; drop the patch when tauri-utils moves to it.) |
| **WebView2 on Windows 7 stops at 109.0.1518.140** and the evergreen bootstrapper is unreliable there (TLS 1.2 often off; it may fetch 110+). Microsoft's download page only offers current versions. | The one official, still-served copy of 109 is the **"Microsoft Edge-WebView2 Runtime Version 109 Update"** package on the Microsoft Update Catalog (the Evergreen Standalone Installer, Microsoft-signed, on the windowsupdate.com CDN). URL + SHA-256 per architecture are pinned in [`src-tauri/webview2-runtime.lock`](src-tauri/webview2-runtime.lock); the workflow downloads it, refuses it unless the hash matches **and** `Get-AuthenticodeSignature` says Microsoft Corporation, and the `tauri.win7-{x64,x86}.conf.json` overlays ship it as a bundle resource (`webviewInstallMode: skip`). [`src-tauri/windows/win7-webview2.nsh`](src-tauri/windows/win7-webview2.nsh) is an NSIS `NSIS_HOOK_POSTINSTALL` that follows Microsoft's documented offline workflow: read the `pv` registry value, run the installer with `/silent /install` only when no runtime exists, then delete the 140 MB file. No manual asset upload anywhere. |

Then [`scripts/audit-win7-imports.ps1`](scripts/audit-win7-imports.ps1) runs `dumpbin /IMPORTS` on the
built exe and fails the job on any DLL or entry point Windows 7 does not have (`combase.dll`,
`bcryptprimitives!ProcessPrng`, `api-ms-win-core-*-l1-2-*`, `SetProcessDpiAwarenessContext`, …). Run it
locally too — it is the difference between "it built" and "it starts on the customer's PC".

**Local spike / repro** (from `src-tauri/`, Developer PowerShell so `dumpbin` is on PATH):

```powershell
rustup toolchain install nightly-2026-08-20 --component rust-src
$env:CARGO_UNSTABLE_BUILD_STD = "std,panic_abort"
# The WebView2 109 standalone installer must ALREADY be at
# src-tauri/webview2-runtime/x64/MicrosoftEdgeWebView2RuntimeInstaller.exe (URL + SHA-256 in
# src-tauri/webview2-runtime.lock) — it is a bundle resource, and tauri-build checks resources exist
# at compile time.
# The merged config MUST reach cargo: generate_context!() bakes the updater endpoint,
# deep-link scheme and icons in at compile time (this is what `tauri build --config` does).
$env:TAURI_CONFIG = (jq -cs '.[0] * .[1]' tauri.business.conf.json tauri.win7-x64.conf.json)
cargo +nightly-2026-08-20 build --release --target x86_64-win7-windows-msvc
..\scripts\audit-win7-imports.ps1 -Exe target\x86_64-win7-windows-msvc\release\orcaa-desktop.exe
# Installer. NOT `tauri build`: the CLI validates --target against `rustup target list`, which
# never lists tier-3 targets, and refuses the triple. `tauri bundle` packages a cargo-built exe.
cd ..; pnpm tauri bundle --target x86_64-win7-windows-msvc --config src-tauri/tauri.business.conf.json --config src-tauri/tauri.win7-x64.conf.json --bundles nsis
```

Spike log (2026-09-04, x64 + x86 debug builds on this recipe): compiles and links; the import audit is
clean apart from the `EventSetInformation` warning above; `tauri bundle` accepts the triple, patches the
exe and derives the `x64`/`x86` arch, and produces the NSIS installer with the WebView2 109 standalone
installer bundled (see the Verification list for what the VM run must still prove).

**Verify on a real Windows 7 SP1 VM before tagging** — a fresh one, without the UCRT update and with TLS
1.2 off, since that is what the field looks like: install, sign in through the browser hand-off, load a
tenant in both themes (the `@supports not (color: color-mix(...))` fallbacks in `shared/styles` are what
keep Chromium 109 from rendering tinted chips as transparent), trigger a notification (banner + taskbar
flash, chime), print a receipt through `spooler.rs`, open an `orcaa://` link, start with `--kiosk`, and
let the updater pull a `latest-win7.json` from a test release.

What Windows 7 users give up, and where it is said out loud: OS toasts (banner + flash instead — the
one-time in-app notice, the downloads page and the FAQ all say so), the "Show in folder" button on a
finished download, and Chromium security updates (109 is end-of-life on Windows 7).

### Manual run (without a tag)

Go to **Actions** tab → **Build & Release Desktop** → **Run workflow**. Uses the version from
`src-tauri/tauri.business.conf.json`, same as a tag push.

---

## macOS-specific notes

**No Mac hardware is needed for releases** — CI builds, signs (Tauri updater key) and uploads the `.dmg` on GitHub's `macos-latest` runner. The sections below only cover Apple's OS-level trust chain and local dev on a Mac.

### Gatekeeper (current state: unsigned)

Until Apple signing is configured, macOS users see _"Orcaa is damaged and can't be opened"_ or _"unidentified developer"_ on first launch (the quarantine flag on downloaded apps). Two documented user workarounds:

- Right-click the app in Applications → **Open** → **Open** (once; remembered afterwards), or
- `xattr -cr /Applications/Orcaa.app`

The landing page's Mac download should link a short "first launch on macOS" note until notarization ships.

### Apple signing + notarization (optional, no Mac needed either)

The workflow auto-enables signing when these repo secrets exist — zero workflow edits:

| Secret                       | Value                                                                         |
| ---------------------------- | ----------------------------------------------------------------------------- |
| `APPLE_CERTIFICATE`          | Base64 of the "Developer ID Application" `.p12` export (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | Password of the `.p12` export                                                 |
| `APPLE_SIGNING_IDENTITY`     | Cert Common Name, e.g. `Developer ID Application: Your Name (TEAMID)`         |
| `APPLE_ID`                   | Apple ID email (enables notarization)                                         |
| `APPLE_PASSWORD`             | App-specific password from appleid.apple.com                                  |
| `APPLE_TEAM_ID`              | 10-char team ID                                                               |

Prereq: [Apple Developer Program](https://developer.apple.com) ($99/yr). The cert can be created and exported through the developer portal in a browser; no Xcode required. With only the first three secrets you get signing (no Gatekeeper "damaged" error but still an internet-download prompt); all six give full notarization (no prompts at all).

### Local dev / manual publish from a Mac (optional)

`pnpm dev:business` / `pnpm build:business` work on a Mac after the prereqs above. [`scripts/publish-update.sh`](scripts/publish-update.sh) can hand-merge a `darwin-*` entry into `latest.json`, but CI's `manifest` job now does this automatically — the script remains for emergency out-of-band patches only.

## Linux-specific notes

- Built on `ubuntu-22.04` **on purpose** — the runner's glibc/webkit2gtk become the minimum for users (Ubuntu 22.04+, Debian 12+, Fedora 38+ and anything newer). Don't bump the runner casually.
- `.deb` / `.rpm` install the app; the **`.AppImage` is the only format the auto-updater can self-update** (deb/rpm users re-download or use a repo). `latest.json` carries a `linux-x86_64` entry pointing at the signed AppImage.
- The tray icon (background mode) uses `libayatana-appindicator3`. On GNOME, users may need the AppIndicator shell extension for the tray to show — standard for all tray apps on GNOME.

---

## Auto-updater

### One-time: generate signing keys

```bash
pnpm tauri signer generate -w ~/.tauri/orcaa.key      # macOS / Linux
pnpm tauri signer generate -w "%USERPROFILE%\.tauri\orcaa.key"   # Windows (CMD)
```

Produces:

- `orcaa.key` — **private**, never commit. Store in 1Password + add to GitHub repo secrets as `TAURI_SIGNING_PRIVATE_KEY`.
- `orcaa.key.pub` — public. Paste the base64 string into [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json) → `plugins.updater.pubkey`.


### Update endpoint

Set in [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json):

```json
"endpoints": [
  "https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/latest.json"
]
```

GitHub auto-redirects `/releases/latest/download/<filename>` to the most recent release's assets. Releases are never marked `prerelease`, so `/latest/` always resolves to the newest one.

**Registering the plugin is not enough — Tauri 2 never checks on its own.** The check is driven from [`src-tauri/src/updater.rs`](src-tauri/src/updater.rs):

- **On launch**, 20 s after startup (window painted, WebSocket up), a silent check runs. It honours snooze and skip (below).
- **On demand**, via the tray's **Check for Updates…**, which ignores snooze and skip — the user just asked — and reports "you're up to date" / "couldn't check" so the click is never a no-op.
- **Before exit**, an `on_before_exit` hook persists the current URL and window geometry. This matters on Windows: `install()` hands off to the NSIS installer and calls `process::exit(0)` itself, so no window event ever fires and unsaved state would be lost.

### The update window

The prompt is a **window the shell draws itself** (`update_page_html`), not a native message box. It is
frameless, parented to the main window, `skip_taskbar(true)`, and positioned over the centre of the app —
clamped through the same work-area helper the main window uses, so an app near a screen edge can't push it
off one. It shows the version pair, the release notes from the manifest (HTML-escaped), a live progress bar,
and three choices:

| Choice                | Effect                                                                                        |
| --------------------- | --------------------------------------------------------------------------------------------- |
| **Install now**       | Downloads and installs. The footer is retired rather than greyed out — once bytes are moving, waiting is the only honest option. |
| **Remind me later**   | Writes `update_snoozed_until` (now + 24 h) to the store. Also what the ✕ and <kbd>Esc</kbd> do. |
| **Skip this version** | Writes `update_skipped_version`. Suppresses that version only; the next one still prompts.       |

It replaced a native `MessageDialogButtons::OkCancelCustom` box that was unbranded and blocking, took its
own taskbar entry (so it read as a second application rather than part of this one), showed no progress, and
— because Win32 falls back to a plain `MessageBox` when it can't raise a TaskDialog — routinely rendered
"Remind me later" as a bare `Cancel`. That dialog survives **only** as a fallback for the case where the
window cannot be created at all: losing the ability to offer an update would be worse than an ugly prompt.

Download progress is pushed into the page with `WebviewWindow::eval` rather than emitted as an event —
`eval` needs no permission, whereas events would drag `core:event` onto a window that otherwise needs almost
nothing. Progress is throttled to whole percent; a fast connection delivers thousands of chunks and one
`eval` each would starve the webview it is trying to animate.

Windows uses NSIS `passive` mode (`/P /R`) — a progress bar, then the installer relaunches the app. macOS and Linux swap the bundle in place and are restarted explicitly via `app.restart()`.

---

## Where state lives on the user's machine

| Data                                                                    | Location (Windows)                                                            |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Cookies, localStorage, IndexedDB, service worker cache (the PWA itself) | `%LOCALAPPDATA%\<identifier>\EBWebView\` (WebView2 user data)                 |
| Last URL, tray-hint-seen flag, update snooze / skipped version (a legacy `zoom_level` from pre-lock versions is deleted at startup) | `%APPDATA%\<identifier>\orcaa-desktop.json` (Tauri store plugin)           |
| Window size / position / maximized                                      | `%APPDATA%\<identifier>\.window-state.json` (window-state plugin)             |
| Shell logs (startup, update checks, failures)                           | `%LOCALAPPDATA%\<identifier>\logs\` — ask users for this file when diagnosing |
| App install (NSIS)                                                      | `%LOCALAPPDATA%\Programs\Orcaa\`                                              |
| App install (MSI per-machine)                                           | `C:\Program Files\Orcaa\`                                                     |

Uninstalling via Apps & Features removes the install dir; WebView2 user data + Tauri store persist unless the user deletes them. Logged-in sessions survive reinstall, matching PWA browser behavior.

---

## Distribution

Installers ship via **GitHub Releases** at [github.com/LogixOrg/orcaa-desktop/releases](https://github.com/LogixOrg/orcaa-desktop/releases).

The [Orcaa landing site](https://orcaa.cloud) download CTAs link to stable filenames that never change across versions:

```
https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/orcaa-desktop.exe           (Windows 10/11, x64)
https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/orcaa-desktop-win7.exe      (Windows 7 SP1, x64 — WebView2 109 bundled)
https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/orcaa-desktop-win7-x86.exe  (Windows 7 SP1, 32-bit)
https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/orcaa-desktop.dmg           (macOS, universal)
https://github.com/LogixOrg/orcaa-desktop/releases/latest/download/orcaa-desktop.AppImage      (Linux)
```

There is no Microsoft Store listing and Windows 7 has no Store — the `.exe` links above are the
distribution channel on Windows.

The GitHub Actions workflow uploads stable-name copies of the versioned installers on each release, so these URLs never change — bumping versions doesn't break the landing page.

---

## Follow-ups (not in v1)

- **Code signing (Windows)** — secrets-only setup, already wired in the workflow via Azure Trusted
  Signing (~$10/mo; see "Enable Windows signing" in `release.yml` for the six secrets). Configure the
  Azure account + app registration and SmartScreen warnings disappear with zero workflow changes.
- **Code signing + notarization (macOS)** — Apple Developer Program ($99/yr). Secrets-only setup, already wired in the workflow (see "Apple signing + notarization" above). Removes Gatekeeper warnings.
- **Camera / microphone permission** — `wry` registers a WebView2 `PermissionRequested` handler **only** when clipboard access is enabled, and even then only auto-allows clipboard-read (`wry/src/webview2/mod.rs:500`); there is no public API for the rest. Mic and camera therefore fall through to WebView2's own prompt, which Chromium persists per origin — so it should ask once and then remember. **Unverified in a real build**: if voice calls or the QR scanner turn out to re-prompt or fail, the options are `--use-fake-ui-for-media-stream` (auto-grants for *every* origin in the webview — not acceptable) or an upstream `wry` change.
- **Custom user agent** — deliberately _not_ set. `user_agent()` replaces the UA string wholesale, and the tenant subdomains sit behind Cloudflare, where a non-standard UA risks bot challenges. The PWA already identifies the shell via `__TAURI_INTERNALS__`; if the backend needs to know, send a header instead.
- **Push when truly quit** — current flow needs the app to be running (background tray is fine). For toasts after Quit, integrate Windows Notification Service (WNS) — needs backend push channel beyond Web Push. Mitigated by the first-run autostart offer.
- **Inline reply on chat toasts** — `tauri-winrt-notification` exposes no text-input API (buttons only); doing this needs hand-built toast XML or an upstream PR. Parked until then.
- **Push when truly quit** (see above) is the remaining notification gap — chat, voice calls and click-to-navigate are all wired now.
