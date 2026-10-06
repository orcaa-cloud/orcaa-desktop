mod i18n;
mod kitchen;
mod label;
mod notify;
mod print;
mod raster;
mod shell_page;
mod signin;
mod payment;
#[cfg(windows)]
mod spooler;
mod updater;

use std::time::Duration;

use serde_json::json;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{DownloadEvent, PageLoadEvent},
    AppHandle, Manager, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_deep_link::DeepLinkExt;
#[cfg(not(legacy_win7))]
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_store::StoreExt;
use tauri_plugin_window_state::{AppHandleExt as WindowStateExt, StateFlags};
use url::Url;

use crate::i18n::Strings;
use crate::shell_page::{
    autostart_prompt_page_html, holding_page_html, loading_page_html, shell_init_js, shell_url,
    update_page_html, ShellPage, SHELL_SCHEME,
};
use crate::signin::PendingSignIn;
use crate::updater::PendingUpdate;

pub(crate) const STORE_FILE: &str = "orcaa-desktop.json";
const STORE_KEY_LAST_URL: &str = "last_url";
const STORE_KEY_TRAY_HINT_SEEN: &str = "tray_hint_seen";
const STORE_KEY_AUTOSTART_ASKED: &str = "autostart_prompt_seen";
/// Legacy — zoom is pinned at 100% now. The key survives only so setup can
/// delete whatever level a pre-lock version persisted.
const STORE_KEY_ZOOM: &str = "zoom_level";

/// Where a fresh install starts. One app serves every audience: auth is the
/// universal sign-in for owners, staff AND platform admins, and it hands the
/// finished session back with the subdomain to land on — `admin` for platform
/// admins, the tenant label for everyone else.
const FALLBACK_URL: &str = "https://auth.orcaa.cloud";

/// The deep-link scheme the browser calls back on. Exactly one desktop app
/// ships, so exactly one scheme is ever registered.
const DEEP_LINK_SCHEME: &str = "orcaa";

const MAIN_WINDOW: &str = "main";

/// Delay before the startup update check. Long enough that the window has
/// painted and the WebSocket has connected, so we're never swapping the binary
/// out from under someone who is still watching the app boot.
const UPDATE_CHECK_DELAY: Duration = Duration::from_secs(20);

/// Size, position and maximized/fullscreen are restored between launches.
/// Visibility deliberately is **not** — the app hides to tray on close, so
/// restoring a "hidden" state would make the next launch appear to do nothing.
fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

/// The auth app is the one orcaa host the webview must never render — sign-in
/// belongs in the user's real browser (see [`signin`]).
fn is_auth_host(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .map(|h| h.starts_with("auth."))
            .unwrap_or(false)
}

/// Any route that would render a sign-in surface inside the app.
///
/// The auth subdomain is the obvious one, but a tenant app also has its **own**
/// `/login` — which is where an expired session or a sign-out lands you. Left
/// alone, the desktop app renders that tenant login form directly, defeating
/// the whole point of moving auth to the browser. Everything here is bounced to
/// the shell's welcome page instead.
pub(crate) fn is_sign_in_route(url: &Url) -> bool {
    if is_auth_host(url) {
        return true;
    }
    if !is_orcaa_host(url) {
        return false;
    }

    let path = url.path().trim_end_matches('/');

    // The handoff landings are how a browser sign-in RETURNS. Bouncing them
    // would make the flow loop forever.
    if path.starts_with("/desktop-handoff") || path.starts_with("/admin-handoff") {
        return false;
    }

    [
        "/login",
        "/register",
        "/forgot-password",
        "/reset-password",
        "/verify-email",
        "/invite",
        "/auth/",
    ]
    .iter()
    .any(|route| path == route.trim_end_matches('/') || path.starts_with(route))
}

/// The registrable domain the tenant subdomains hang off, derived from whatever
/// host the app is configured against so dev (`orcaa.test`) works unchanged.
fn base_domain_of(url: &Url) -> String {
    url.host_str()
        .map(|host| {
            let labels: Vec<&str> = host.split('.').collect();
            if labels.len() >= 2 {
                labels[labels.len() - 2..].join(".")
            } else {
                host.to_string()
            }
        })
        .unwrap_or_else(|| "orcaa.cloud".to_string())
}

/// A real Orcaa web address — as opposed to the shell's own scaffolding page.
///
/// Must stay in agreement with `capabilities/remote.json` — a host that
/// capability grants IPC to but this function rejects gets bounced to the
/// system browser before its page can ever call anything.
pub(crate) fn is_orcaa_host(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .map(|h| {
                h == "orcaa.cloud"
                    || h.ends_with(".orcaa.cloud")
                    || h == "orcaa.test"
                    || h.ends_with(".orcaa.test")
            })
            .unwrap_or(false)
}

/// The shell's own pages, which arrive as `http://orcaa-shell.localhost` on
/// Windows and `orcaa-shell://` elsewhere.
fn is_shell_url(url: &Url) -> bool {
    url.scheme() == SHELL_SCHEME || url.host_str() == Some(&format!("{SHELL_SCHEME}.localhost"))
}

fn is_internal_url(url: &Url) -> bool {
    // The shell pages must be recognised here, or `on_navigation` would hand the
    // app's own UI to the system browser.
    is_orcaa_host(url)
        || is_shell_url(url)
        || matches!(url.scheme(), "about" | "data" | "blob" | "tauri")
}

// ---------------------------------------------------------------------------
// Window geometry
// ---------------------------------------------------------------------------

/// Fits a window rectangle inside a monitor's work area.
///
/// Both rectangles are `(x, y, width, height)` in physical pixels. Size is
/// clamped first, because a window wider than the screen has no position that
/// would fit it.
///
/// This exists because `tauri-plugin-window-state` restores a saved position
/// verbatim whenever the saved rectangle *intersects* any monitor — mere
/// intersection, not containment. A window dragged until its title bar sat above
/// the top edge of the screen therefore came back exactly like that on the next
/// launch, with the minimise/restore/close buttons sliced in half and no way to
/// grab the title bar to fix it.
pub(crate) fn clamped_rect(
    window: (i32, i32, u32, u32),
    area: (i32, i32, u32, u32),
) -> (i32, i32, u32, u32) {
    let (x, y, w, h) = window;
    let (ax, ay, aw, ah) = area;

    let w = w.min(aw);
    let h = h.min(ah);

    // `max` before `min` so a work area smaller than the window still pins the
    // window's top-left corner on screen rather than off the far edge.
    let x = x.min(ax + aw as i32 - w as i32).max(ax);
    let y = y.min(ay + ah as i32 - h as i32).max(ay);

    (x, y, w, h)
}

/// Pulls a window fully onto the work area of the monitor it sits on.
///
/// Uses the *work* area, not the full monitor bounds, so the window never hides
/// under the taskbar either.
fn clamp_window_to_work_area(window: &WebviewWindow) {
    // Maximised and fullscreen geometry belongs to the OS; touching it would
    // un-maximise the window.
    if window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return;
    }

    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };

    // Resolved from the window's centre, not its origin: an origin that is
    // already off-screen resolves to the wrong monitor, or to none at all.
    let cx = pos.x as f64 + size.width as f64 / 2.0;
    let cy = pos.y as f64 + size.height as f64 / 2.0;

    let monitor = match window.monitor_from_point(cx, cy) {
        Ok(Some(monitor)) => Some(monitor),
        _ => window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten()),
    };

    let Some(monitor) = monitor else {
        return;
    };
    let work = monitor.work_area();

    let (x, y, w, h) = clamped_rect(
        (pos.x, pos.y, size.width, size.height),
        (
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height,
        ),
    );

    // Size first: on Windows a resize can shift the origin, so setting the
    // position afterwards is what makes the result stick.
    if (w, h) != (size.width, size.height) {
        let _ = window.set_size(tauri::PhysicalSize::new(w, h));
    }
    if (x, y) != (pos.x, pos.y) {
        log::info!(
            "pulled the window back on screen: ({},{}) -> ({x},{y})",
            pos.x,
            pos.y
        );
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Windows 7 has no toast centre, so "something happened" is signalled the way
/// every classic app does it: the taskbar button flashes until the window is
/// brought forward. Only the taskbar button — the window itself is left where
/// it is, so a cashier mid-sale is never interrupted. The web app's in-app
/// banner and chime carry the actual message.
#[cfg(all(windows, legacy_win7))]
pub(crate) fn flash_main_window(app: &AppHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG,
    };

    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Ok(hwnd) = window.hwnd() else {
        return;
    };

    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    // SAFETY: `info` is a fully initialised FLASHWINFO whose hwnd came from the
    // live window; FlashWindowEx only reads it. The return value is the
    // window's previous active state — nothing to act on.
    let _ = unsafe { FlashWindowEx(&info) };
}

/// The global summon shortcut's behaviour: bring the app forward, unless it is
/// already the thing in front — then put it away again.
fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);

    if visible && focused {
        let _ = window.hide();
    } else {
        show_main_window(app);
    }
}

/// Tray quick actions: raise the window and jump to an app page.
///
/// Only ever swaps the *path* on the origin the webview already sits on, so a
/// tray item can never move the app to another host. Before sign-in the webview
/// is on a shell page with no tenant origin — the action then just raises the
/// window, which is the most useful thing it can do there.
fn open_app_path(app: &AppHandle, path: &str) {
    show_main_window(app);

    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Ok(current) = window.url() else {
        return;
    };
    if !is_orcaa_host(&current) {
        return;
    }

    let mut url = current.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);

    if url != current {
        if let Err(err) = window.navigate(url) {
            log::error!("tray navigation failed: {err}");
        }
    }
}

/// Whether a URL is the platform console rather than a tenant app.
///
/// One desktop app serves both audiences, so anything that used to differ
/// between the two builds is now resolved from the page actually on screen.
fn is_admin_console(url: &Url) -> bool {
    is_orcaa_host(url)
        && url
            .host_str()
            .map(|host| host.starts_with("admin."))
            .unwrap_or(false)
}

fn showing_admin_console(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN_WINDOW)
        .and_then(|window| window.url().ok())
        .map(|url| is_admin_console(&url))
        .unwrap_or(false)
}

/// Tray handle for the primary quick-jump, managed so its label can follow the
/// origin the webview lands on.
struct PrimaryNavMenuItem(MenuItem<tauri::Wry>);

/// Keeps the tray's primary jump honest: "POS" on a tenant, "Today" on the
/// platform console. A menu entry promising POS that lands on `admin/pos` (a
/// 404) is worse than no entry at all.
fn sync_primary_nav_item(app: &AppHandle, strings: &Strings) {
    // The tray is built after the window, so the earliest page loads find no
    // item yet — setup re-syncs once the menu exists.
    let Some(item) = app.try_state::<PrimaryNavMenuItem>() else {
        return;
    };

    let label = if showing_admin_console(app) {
        strings.tray_today()
    } else {
        strings.tray_pos()
    };

    if let Err(err) = item.0.set_text(label) {
        log::warn!("failed to update the tray quick-jump label: {err}");
    }
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

/// Hands sign-in to the system browser and switches the window to the waiting
/// state.
///
/// Reachable **only** through the `signin_start` command, which the welcome
/// page fires from a click handler. It used to be triggered by navigating to a
/// magic `?action=signin` URL that `on_navigation` watched for, which meant
/// anything that produced that navigation opened the browser — including simply
/// landing on the page after a sign-out or a cold launch.
fn start_browser_sign_in(app: &AppHandle) {
    let Some(config) = app.try_state::<AppUrls>() else {
        return;
    };
    let pending = app.state::<PendingSignIn>();

    let Some(browser_url) = pending.begin(&config.auth_base, DEEP_LINK_SCHEME) else {
        log::error!("failed to start browser sign-in");
        return;
    };

    log::info!("handing sign-in to the system browser (user pressed the button)");

    if let Err(err) = app.opener().open_url(browser_url.to_string(), None::<&str>) {
        log::error!("failed to open the browser for sign-in: {err}");
    }

    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(err) = window.navigate(shell_url(&ShellPage::Waiting, true)) {
            log::error!("failed to show the waiting page: {err}");
        }
    }
}

/// Completes a sign-in that came back over `orcaa://`, by pointing the webview
/// at the tenant's handoff route with the ticket and the verifier.
fn complete_browser_sign_in(app: &AppHandle, incoming: &Url) {
    let Some(config) = app.try_state::<AppUrls>() else {
        return;
    };

    let Some(resolved) =
        app.state::<PendingSignIn>()
            .resolve(incoming, &config.base_domain, DEEP_LINK_SCHEME)
    else {
        // Unsolicited, replayed, or tampered — indistinguishable on purpose.
        log::warn!("ignoring a deep link that does not match a pending sign-in");
        return;
    };

    log::info!(
        "resuming sign-in on {}",
        resolved.url.host_str().unwrap_or("?")
    );

    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(err) = window.navigate(resolved.url) {
            log::error!("failed to resume sign-in: {err}");
        }
    }

    show_main_window(app);
}

/// The hosts this build talks to, resolved once from config so dev
/// (`*.orcaa.test`) needs no special-casing downstream.
struct AppUrls {
    auth_base: String,
    base_domain: String,
}

#[cfg(not(legacy_win7))]
pub(crate) fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        log::warn!("failed to show notification: {err}");
    }
}

/// Windows 7: no notification plugin in this build (it binds WinRT). The
/// shell's own messages — update failed, download failed — go to the log and
/// flash the taskbar; the update window itself still shows its own state.
#[cfg(legacy_win7)]
pub(crate) fn notify(app: &AppHandle, title: &str, body: &str) {
    log::info!("notification (no toast centre on Windows 7): {title} — {body}");
    #[cfg(windows)]
    flash_main_window(app);
    #[cfg(not(windows))]
    let _ = app;
}

/// Persists everything the next launch needs to feel continuous: the page the
/// user was on, plus the window geometry.
///
/// Called from every exit path — the close button, the tray's Quit, and the
/// updater's pre-exit hook. That last one matters: on Windows the updater
/// hands off to the NSIS installer and calls `process::exit(0)` itself, so no
/// window event ever fires and anything not saved here is lost.
pub(crate) fn persist_session(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if let Ok(url) = window.url() {
            // Only ever remember a real page the user was working on. Saving
            // the shell's own scaffolding would reopen the app on its waiting
            // screen instead of the welcome screen, and saving the auth host is
            // pointless — the fallback already resolves there.
            if is_orcaa_host(&url) && !is_sign_in_route(&url) {
                if let Ok(store) = app.store(STORE_FILE) {
                    store.set(STORE_KEY_LAST_URL, json!(url.to_string()));
                    let _ = store.save();
                }
            }
        }
    }

    if let Err(err) = app.save_window_state(window_state_flags()) {
        log::warn!("failed to save window state: {err}");
    }
}

/// One-time explanation of where the app went when the user first clicks X.
/// Hiding to tray is the right default (it keeps the WebSocket alive so
/// notifications keep arriving) but it is surprising the first time.
fn maybe_show_tray_hint(app: &AppHandle, strings: &Strings) {
    let Ok(store) = app.store(STORE_FILE) else {
        return;
    };

    let already_seen = store
        .get(STORE_KEY_TRAY_HINT_SEEN)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if already_seen {
        return;
    }

    store.set(STORE_KEY_TRAY_HINT_SEEN, json!(true));
    let _ = store.save();

    notify(app, &strings.tray_hint_title(), &strings.tray_hint_body());
}

// ---------------------------------------------------------------------------
// Zoom — pinned at 100%
// ---------------------------------------------------------------------------

/// Re-applied after every page load: WebView2 keeps a host-level zoom factor
/// per profile (a user may have zoomed before this lock shipped, or the OS may
/// restore one), so pinning once at startup would not hold across navigations.
/// The UI is designed for a fixed 100% zoom — the app's own font-scale
/// preference is the sanctioned knob.
fn reset_zoom(window: &WebviewWindow) {
    let _ = window.set_zoom(1.0);
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn signin_start(app: AppHandle) {
    start_browser_sign_in(&app);
}

#[tauri::command]
fn shell_reload(window: WebviewWindow) {
    let _ = window.reload();
}

#[tauri::command]
fn shell_fullscreen_toggle(window: WebviewWindow) {
    let is_fullscreen = window.is_fullscreen().unwrap_or(false);
    let _ = window.set_fullscreen(!is_fullscreen);
}

#[tauri::command]
fn shell_quit(app: AppHandle) {
    persist_session(&app);
    app.exit(0);
}

// ---------------------------------------------------------------------------
// Custom titlebar (Windows only — macOS keeps its traffic lights via the
// Overlay titlebar style, Linux keeps native decorations because undecorated
// GTK windows lose their resize edges)
// ---------------------------------------------------------------------------

/// The window buttons the web topbar draws when the OS frame is gone.
///
/// Scoped to the main window: the update window has its own chrome, and the
/// action set is fixed so the remote page can never do more than the three
/// caption buttons could.
#[tauri::command]
fn shell_window_control(window: WebviewWindow, action: String) {
    if window.label() != MAIN_WINDOW {
        return;
    }

    match action.as_str() {
        "minimize" => {
            let _ = window.minimize();
        }
        "toggle-maximize" => {
            if window.is_maximized().unwrap_or(false) {
                let _ = window.unmaximize();
            } else {
                let _ = window.maximize();
            }
        }
        // `close()` (not `destroy()`) so this rides the CloseRequested handler
        // and hides to the tray exactly like the native X did.
        "close" => {
            let _ = window.close();
        }
        other => log::warn!("ignoring unknown window control action: {other}"),
    }
}

/// Lets the web titlebar draw the right maximize/restore glyph. Polled on the
/// DOM `resize` event rather than pushed — the state only matters while the
/// user is looking at the button.
#[tauri::command]
fn shell_window_state(window: WebviewWindow) -> serde_json::Value {
    json!({
        "maximized": window.is_maximized().unwrap_or(false),
        "fullscreen": window.is_fullscreen().unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------
// First-run autostart offer — a branded window, never the OS message box
// ---------------------------------------------------------------------------

const AUTOSTART_WINDOW: &str = "autostart";

/// Tray checkbox handle, managed so the offer window's commands can keep the
/// menu in step with what they just changed.
struct AutostartMenuItem(CheckMenuItem<tauri::Wry>);

fn autostart_window_builder(
    app: &AppHandle,
    title: String,
) -> tauri::WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    tauri::WebviewWindowBuilder::new(
        app,
        AUTOSTART_WINDOW,
        tauri::WebviewUrl::CustomProtocol(shell_url(&ShellPage::AutostartPrompt, false)),
    )
    .title(title)
    .inner_size(460.0, 330.0)
    .resizable(false)
    .minimizable(false)
    .maximizable(false)
    .decorations(false)
    .shadow(true)
    .skip_taskbar(true)
    .focused(true)
    // Revealed on first paint, same as the update window — no white flash.
    .visible(false)
    .on_page_load(|window, payload| {
        if matches!(payload.event(), PageLoadEvent::Finished) {
            updater::center_on_main(&window.app_handle().clone(), &window);
            let _ = window.show();
            let _ = window.set_focus();
        }
    })
}

fn open_autostart_prompt(app: &AppHandle) {
    if app.get_webview_window(AUTOSTART_WINDOW).is_some() {
        return;
    }

    let title = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| "Orcaa".into());

    // Parent to the main window when possible (updater pattern); an
    // unparented offer is still far better than none.
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        match autostart_window_builder(app, title.clone()).parent(&main) {
            Ok(builder) => {
                if let Err(err) = builder.build() {
                    log::warn!("failed to open the autostart offer: {err}");
                }
                return;
            }
            Err(err) => {
                log::warn!("could not parent the autostart offer, opening it standalone: {err}")
            }
        }
    }

    if let Err(err) = autostart_window_builder(app, title).build() {
        log::warn!("failed to open the autostart offer: {err}");
    }
}

fn close_autostart_prompt(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(AUTOSTART_WINDOW) {
        let _ = window.close();
    }
}

#[tauri::command]
fn autostart_accept(app: AppHandle) {
    let manager = app.autolaunch();
    if let Err(err) = manager.enable() {
        log::error!("failed to enable autostart from the offer: {err}");
    }
    // The tray checkbox is the permanent control — keep it truthful.
    if let Some(item) = app.try_state::<AutostartMenuItem>() {
        let _ = item.0.set_checked(manager.is_enabled().unwrap_or(false));
    }
    close_autostart_prompt(&app);
}

#[tauri::command]
fn autostart_decline(app: AppHandle) {
    close_autostart_prompt(&app);
}

// ---------------------------------------------------------------------------
// Unread badge
// ---------------------------------------------------------------------------

/// A red dot for the Windows taskbar overlay, drawn in code so no asset or
/// image crate is needed. Windows renders it at 16x16 in the icon's corner —
/// a count would be unreadable at that size, so presence is the signal.
#[cfg(windows)]
fn badge_overlay_icon() -> tauri::image::Image<'static> {
    const SIZE: u32 = 32;
    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = SIZE as f32 * 0.42;

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            // One-pixel soft edge so the dot doesn't alias into a blob.
            let alpha = ((radius - distance + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i] = 0xe8;
            rgba[i + 1] = 0x11;
            rgba[i + 2] = 0x23;
            rgba[i + 3] = alpha;
        }
    }

    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

/// Mirrors the web app's unread notification count onto the taskbar/dock.
///
/// macOS/Linux get a real number on the dock icon; Windows has no numeric
/// badge API, so any non-zero count shows a red-dot overlay. Zero clears.
#[tauri::command]
fn shell_badge(window: WebviewWindow, count: u64) {
    if window.label() != MAIN_WINDOW {
        return;
    }

    #[cfg(windows)]
    {
        let icon = if count == 0 {
            None
        } else {
            Some(badge_overlay_icon())
        };
        if let Err(err) = window.set_overlay_icon(icon) {
            log::warn!("failed to set the taskbar badge: {err}");
        }
    }

    #[cfg(not(windows))]
    {
        let badge = if count == 0 { None } else { Some(count as i64) };
        if let Err(err) = window.set_badge_count(badge) {
            log::warn!("failed to set the dock badge: {err}");
        }
    }
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

/// Reports a finished download the way a desktop app should.
///
/// Every export in the app (xlsx, PDF, JSON) is a blob plus a synthetic
/// `<a download>`, so WebView2 handles it with Edge's own download flyout and
/// drops the file in Downloads. That works, but it is unmistakably a browser
/// behaviour and it leaves the user hunting for the file. The download itself is
/// left alone — only the ending is ours: a toast naming the file, with a button
/// that opens the folder with it selected.
fn on_download_event(app: &AppHandle, event: DownloadEvent<'_>) -> bool {
    let DownloadEvent::Finished { path, success, .. } = event else {
        // `Requested` — returning true lets WebView2 save where it normally
        // would. Redirecting it would surprise anyone who has set a download
        // folder in Edge.
        return true;
    };

    let strings = app
        .try_state::<Strings>()
        .map(|s| (*s).clone())
        .unwrap_or_else(|| Strings::detect("Orcaa".to_string()));

    let Some(path) = path.filter(|_| success) else {
        if !success {
            log::warn!("a download failed");
            notify(
                app,
                &strings.download_failed_title(),
                &strings.update_failed_body(),
            );
        }
        return true;
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    log::info!("download finished: {}", path.display());
    reveal_toast(app, &strings, &name, path);
    true
}

#[cfg(all(windows, not(legacy_win7)))]
fn reveal_toast(app: &AppHandle, strings: &Strings, name: &str, path: std::path::PathBuf) {
    use tauri_winrt_notification::Toast;

    let handle = app.clone();
    let toast = Toast::new(&app.config().identifier)
        .title(&strings.download_done_title())
        .text1(name)
        .add_button(&strings.download_reveal(), "reveal")
        .on_activated(move |_| {
            if let Err(err) = handle.opener().reveal_item_in_dir(&path) {
                log::warn!("failed to reveal the download: {err}");
            }
            Ok(())
        });

    if toast.show().is_err() {
        notify(app, &strings.download_done_title(), name);
    }
}

/// macOS/Linux, and Windows 7 (no WinRT toast, so no "Show in folder"
/// button — the file still lands in Downloads as usual).
#[cfg(any(not(windows), legacy_win7))]
fn reveal_toast(app: &AppHandle, strings: &Strings, name: &str, _path: std::path::PathBuf) {
    notify(app, &strings.download_done_title(), name);
}

// ---------------------------------------------------------------------------

pub fn run() {
    // Release builds abort on panic (`panic = "abort"`), and the default hook
    // prints to a stderr nobody sees on a double-clicked Windows app. Leave a
    // trace in the log file first, or a crash at a customer's counter is
    // invisible. Registered before the builder so even setup panics are
    // caught; if the log plugin isn't up yet the record is simply dropped —
    // no worse than before.
    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC (shell is about to abort): {info}");
    }));

    let builder = tauri::Builder::default()
        // Serves the shell's own pages. Everything they need is derived from
        // config or managed state here rather than carried in the URL, so a
        // stale URL can never describe something that is no longer true.
        .register_uri_scheme_protocol(SHELL_SCHEME, |ctx, request| {
            let app = ctx.app_handle();
            let product = app
                .config()
                .product_name
                .clone()
                .unwrap_or_else(|| "Orcaa".into());
            let strings = Strings::detect(product);

            let page = ShellPage::from_query(request.uri().query().unwrap_or_default());

            let html = match page {
                ShellPage::Welcome => holding_page_html(&strings, false),
                ShellPage::Waiting => holding_page_html(&strings, true),
                ShellPage::Loading { ref target } => loading_page_html(&strings, target),
                ShellPage::Update => match app.try_state::<PendingUpdate>().and_then(|s| s.get()) {
                    Some(update) => update_page_html(
                        &strings,
                        &update.current_version,
                        &update.version,
                        update.body.as_deref(),
                    ),
                    // Reloading the window after the offer was withdrawn: show
                    // the welcome page rather than an empty update prompt.
                    None => holding_page_html(&strings, false),
                },
                ShellPage::AutostartPrompt => autostart_prompt_page_html(&strings),
            };

            // The config-level `app.security.csp` never reaches these pages —
            // it only decorates the asset protocol, and this app has no
            // `frontendDist` — so the policy rides the response header instead.
            // Everything a shell page uses is inline (styles, scripts, the SVG
            // logo); the only network egress is the reachability probe against
            // the tenant host, plus Tauri's own IPC transport, which is a
            // `fetch` to `ipc:`/`http://ipc.localhost` on macOS and Linux.
            // Rust-side `eval` (updater progress) executes outside page CSP.
            tauri::http::Response::builder()
                .header(
                    tauri::http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8",
                )
                .header(
                    tauri::http::header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; style-src 'unsafe-inline'; \
                     script-src 'unsafe-inline'; img-src data:; \
                     connect-src ipc: http://ipc.localhost \
                     https://orcaa.cloud https://*.orcaa.cloud \
                     http://orcaa.test http://*.orcaa.test \
                     https://orcaa.test https://*.orcaa.test; \
                     base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
                )
                .body(html.into_bytes())
                .expect("shell page response must build")
        })
        // Must be registered FIRST. On Windows an `orcaa://` link launches a
        // whole new process with the URL in argv; the single-instance plugin's
        // `deep-link` feature forwards it into the running app so `on_open_url`
        // fires there instead of a second window appearing.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: None,
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags())
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        // Opt-in, off until the user ticks the tray item. `--hidden` so a
        // machine that boots straight into Orcaa lands in the tray rather than
        // throwing a window in front of whoever is signing in.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ));

    // Not compiled into the Windows 7 build: the plugin's Windows backend is
    // WinRT (`combase.dll`), which Windows 7 does not have — linking it would
    // stop the exe from loading at all. notify.rs carries the Win7 fallback.
    #[cfg(not(legacy_win7))]
    let builder = builder.plugin(tauri_plugin_notification::init());

    builder
        .invoke_handler(tauri::generate_handler![
            payment::shell_payment_register,
            signin_start,
            shell_reload,
            shell_fullscreen_toggle,
            shell_quit,
            shell_window_control,
            shell_window_state,
            shell_badge,
            autostart_accept,
            autostart_decline,
            print::shell_pos_print,
            print::shell_pos_test_print,
            print::shell_pos_printer_get,
            print::shell_pos_printer_set,
            print::shell_pos_printers_list,
            print::shell_pos_printer_autodetect,
            label::shell_label_print,
            label::shell_label_test_print,
            label::shell_label_printer_get,
            label::shell_label_printer_set,
            label::shell_label_printer_autodetect,
            label::shell_label_calibrate,
            kitchen::shell_kot_print,
            kitchen::shell_kitchen_test_print,
            kitchen::shell_kitchen_printers_get,
            kitchen::shell_kitchen_printer_set,
            kitchen::shell_kitchen_printer_autodetect,
            raster::shell_print_raster,
            notify::shell_notify,
            updater::update_install,
            updater::update_snooze,
            updater::update_skip,
        ])
        .setup(|app| {
            let identifier = app.config().identifier.clone();
            let product_name = app
                .config()
                .product_name
                .clone()
                .unwrap_or_else(|| "Orcaa".into());
            let version = app.package_info().version.to_string();
            let strings = Strings::detect(product_name.clone());

            log::info!("starting {product_name} {version} ({identifier})");

            let store = app.store(STORE_FILE)?;

            // Zoom is pinned at 100% now — drop whatever level a pre-lock
            // version persisted so an upgraded install can never come back up
            // at 80%/140%.
            if store.delete(STORE_KEY_ZOOM) {
                let _ = store.save();
            }

            let saved = store
                .get(STORE_KEY_LAST_URL)
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| FALLBACK_URL.to_string());

            let initial_url: Url = saved
                .parse()
                .or_else(|_| FALLBACK_URL.parse())
                .expect("fallback URL must parse");

            let fallback_url: Url = FALLBACK_URL.parse().expect("fallback URL must parse");
            // auth.* is the ONLY sign-in surface, for every guard including
            // platform admins — the admin app ships a /login shim that forwards
            // here, and auth hands admins back under the reserved "admin" label.
            app.manage(AppUrls {
                auth_base: format!("https://auth.{}", base_domain_of(&fallback_url)),
                base_domain: base_domain_of(&fallback_url),
            });
            app.manage(PendingSignIn::default());
            app.manage(payment::PendingPayment::default());
            app.manage(PendingUpdate::default());
            app.manage(strings.clone());

            // The saved URL is the page the user was last on, which may well be
            // the auth app. Sign-in never renders in here, so park on the
            // welcome page and wait for the user to ask.
            //
            // Otherwise start on the boot page, which shows instantly and hands
            // off to the tenant once it has confirmed the host answers. The
            // window used to be created hidden and revealed on first paint,
            // which meant a slow or unreachable backend showed nothing at all.
            let start_page = if is_sign_in_route(&initial_url) {
                ShellPage::Welcome
            } else {
                ShellPage::Loading {
                    target: initial_url.to_string(),
                }
            };

            let nav_handle = app.handle().clone();
            let load_product = product_name.clone();
            let load_strings = strings.clone();

            let window_builder = tauri::WebviewWindowBuilder::new(
                app,
                MAIN_WINDOW,
                tauri::WebviewUrl::CustomProtocol(shell_url(&start_page, false)),
            )
            .title(&product_name)
            .inner_size(1440.0, 900.0)
            .min_inner_size(1024.0, 640.0)
            .resizable(true)
            // Only affects a first launch — the window-state plugin overrides
            // this whenever it has geometry to restore.
            .center()
            // Tauri's own drag-and-drop handler is on by default and swallows
            // HTML5 file drops before the page ever sees them, which silently
            // breaks every upload dropzone in the app. The shell listens for no
            // drag events of its own, so there is nothing to lose.
            .disable_drag_drop_handler()
            // Chromium blocks audio that no user gesture asked for. The app
            // plays a sound when a notification arrives and rings on an incoming
            // call — precisely the moments when nobody has touched the window,
            // and often when it is hidden in the tray. Without this the sound is
            // dropped silently and the toast arrives in silence.
            //
            // Note this REPLACES wry's own defaults, so they are repeated here:
            // dropping them would restore Edge's "mini menu" and run WebView2's
            // SmartScreen over the app's own pages.
            .additional_browser_args(
                "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
                 --autoplay-policy=no-user-gesture-required",
            )
            .initialization_script(shell_init_js(
                &strings,
                tauri::webview_version().ok().as_deref(),
            ))
            .on_download(|webview, event| on_download_event(&webview.app_handle().clone(), event))
            .on_navigation(move |url| {
                // Logging out, or a session expiring, lands here. Show the
                // welcome page and stop — hijacking someone's browser the
                // instant they sign out is hostile. They asked to leave.
                if is_sign_in_route(url) {
                    let handle = nav_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
                            let _ = window.navigate(shell_url(&ShellPage::Welcome, true));
                        }
                    });
                    return false;
                }

                if is_internal_url(url) {
                    return true;
                }

                // Everything else is a link out of the app, which belongs in the
                // user's browser — but only ever as the result of them clicking
                // something. A redirect chain during boot must never be able to
                // launch a browser on its own, so external URLs are refused
                // until the app itself is on screen.
                let ready = nav_handle
                    .get_webview_window(MAIN_WINDOW)
                    .and_then(|w| w.url().ok())
                    .map(|current| is_orcaa_host(&current))
                    .unwrap_or(false);

                if ready {
                    log::info!("opening {url} in the system browser");
                    let _ = nav_handle.opener().open_url(url.to_string(), None::<&str>);
                } else {
                    log::warn!("refused to open {url} externally before the app had loaded");
                }
                false
            })
            .on_page_load(move |window, payload| {
                if !matches!(payload.event(), PageLoadEvent::Finished) {
                    return;
                }

                log::info!("page loaded: {}", payload.url());
                reset_zoom(&window);
                set_window_title(&window, &load_product);
                sync_primary_nav_item(&window.app_handle().clone(), &load_strings);
            });

            // Branded titlebar. Windows drops the OS frame — the web topbar
            // draws the caption buttons via `shell_window_control` and drags
            // via `data-tauri-drag-region` (shadow(true) keeps the resize
            // borders and edge-snap). macOS keeps its native traffic lights
            // floating over the page instead: custom buttons there would lose
            // the fullscreen/Stage Manager behaviours. Linux stays fully
            // native — undecorated GTK windows have no resize edges.
            #[cfg(windows)]
            let window_builder = window_builder.decorations(false).shadow(true);
            #[cfg(target_os = "macos")]
            let window_builder = window_builder
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true);

            let window = window_builder.build()?;

            // Runs after the window-state plugin's `on_window_ready` hook, so
            // this sees the geometry the user is actually about to get.
            clamp_window_to_work_area(&window);

            // Kiosk / station mode: a counter PC that boots straight into a
            // fullscreen Orcaa (pair with autostart). Fullscreen also retires
            // the injected titlebar strip, so the page owns every pixel. Exit
            // stays where it always was — the tray's Quit.
            if std::env::args().any(|arg| arg == "--kiosk") {
                let _ = window.set_fullscreen(true);
            }

            // The browser hands the finished session back here.
            let deep_link_handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    payment::complete(&deep_link_handle, &url);
                    complete_browser_sign_in(&deep_link_handle, &url);
                }
            });

            // A cold start *from* a deep link (app not running when the browser
            // handed back) delivers the URL through `get_current` rather than
            // the event — but only a link this process initiated can resolve,
            // so a launch-by-link with no pending attempt is correctly inert.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                let cold_handle = app.handle().clone();
                for url in urls {
                    payment::complete(&cold_handle, &url);
                    complete_browser_sign_in(&cold_handle, &url);
                }
            }

            // Global summon shortcut — the app lives in the tray, so it needs a
            // key that works from anywhere. Ctrl(⌘)+Shift+O. A registration
            // conflict with some other program is logged and ignored — never
            // fatal.
            {
                use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

                let modifiers = if cfg!(target_os = "macos") {
                    Modifiers::SUPER | Modifiers::SHIFT
                } else {
                    Modifiers::CONTROL | Modifiers::SHIFT
                };
                let summon = Shortcut::new(Some(modifiers), Code::KeyO);

                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app, shortcut, event| {
                            if event.state() == ShortcutState::Pressed && shortcut == &summon {
                                toggle_main_window(app);
                            }
                        })
                        .build(),
                )?;

                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                if let Err(err) = app.global_shortcut().register(summon) {
                    log::warn!("summon shortcut unavailable (taken by another app?): {err}");
                }
            }

            let show_item =
                MenuItem::with_id(app, "show", strings.tray_open(), true, None::<&str>)?;
            // Quick jumps to the two pages each audience reaches for most. The
            // handler only ever swaps the path on the current origin (see
            // `open_app_path`), so these are navigation sugar, not new powers.
            //
            // One app now serves both the tenant and the platform console, so
            // the primary jump follows the origin currently on screen rather
            // than a build flag: POS for a tenant, Today for admin.
            // `sync_primary_nav_item` keeps the label truthful on navigation.
            let quick_primary =
                MenuItem::with_id(app, "nav_primary", strings.tray_pos(), true, None::<&str>)?;
            let quick_dashboard = MenuItem::with_id(
                app,
                "nav_dashboard",
                strings.tray_dashboard(),
                true,
                None::<&str>,
            )?;
            let update_item = MenuItem::with_id(
                app,
                "check_updates",
                strings.tray_check_updates(),
                true,
                None::<&str>,
            )?;
            // The tray is the right home for this: the whole point of the
            // setting is that the app is already sitting there when the machine
            // comes up, and it needs no round trip through the web app.
            let autostart_item = CheckMenuItem::with_id(
                app,
                "autostart",
                strings.tray_autostart(),
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
            let quit_item =
                MenuItem::with_id(app, "quit", strings.tray_quit(), true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_item,
                    &PredefinedMenuItem::separator(app)?,
                    &quick_primary,
                    &quick_dashboard,
                    &PredefinedMenuItem::separator(app)?,
                    &autostart_item,
                    &update_item,
                    &PredefinedMenuItem::separator(app)?,
                    &quit_item,
                ],
            )?;

            // The branded first-run offer's accept command flips this checkbox
            // — hand it a clone through managed state before the menu-event
            // closure consumes the original.
            app.manage(AutostartMenuItem(autostart_item.clone()));

            // Same handle-through-managed-state trick, so `sync_primary_nav_item`
            // can relabel the jump as the webview moves between the tenant app
            // and the platform console. Synced once here because the first page
            // load happens before this menu exists.
            app.manage(PrimaryNavMenuItem(quick_primary.clone()));
            sync_primary_nav_item(app.handle(), &strings);

            let menu_strings = strings.clone();
            // Product name only — no version. The hover tooltip is a brand
            // surface, not a diagnostic one; the version lives in the startup
            // log and the update prompt.
            let mut tray_builder = TrayIconBuilder::with_id("main")
                .tooltip(&product_name)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    // Admin's Today command center lives at "/", not "/today".
                    "nav_primary" => {
                        let path = if showing_admin_console(app) {
                            "/"
                        } else {
                            "/pos"
                        };
                        open_app_path(app, path)
                    }
                    "nav_dashboard" => open_app_path(app, "/dashboard"),
                    "autostart" => {
                        let manager = app.autolaunch();
                        let enabled = manager.is_enabled().unwrap_or(false);
                        let result = if enabled {
                            manager.disable()
                        } else {
                            manager.enable()
                        };

                        match result {
                            // The checkbox is re-synced from the real state
                            // rather than toggled optimistically — a blocked
                            // registry write must not leave the menu claiming
                            // something that isn't true.
                            Ok(()) => log::info!(
                                "autostart {}",
                                if enabled { "disabled" } else { "enabled" }
                            ),
                            Err(err) => log::error!("failed to change autostart: {err}"),
                        }
                        let _ = autostart_item.set_checked(manager.is_enabled().unwrap_or(false));
                    }
                    "check_updates" => {
                        let handle = app.clone();
                        let strings = menu_strings.clone();
                        tauri::async_runtime::spawn(async move {
                            updater::check_for_updates(handle, strings, true).await;
                        });
                    }
                    "quit" => {
                        persist_session(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }

            tray_builder.build(app)?;

            // One-time autostart offer — a BRANDED shell window (same chrome
            // as the update prompt), never the OS's stock message box. Asked
            // once; the tray checkbox stays the permanent control either way.
            // Skipped when launched `--hidden` (autostart already did its job)
            // or when autostart is already on.
            {
                let launched_hidden = std::env::args().any(|arg| arg == "--hidden");
                let already_enabled = app.autolaunch().is_enabled().unwrap_or(false);
                if !launched_hidden && !already_enabled {
                    if let Ok(store) = app.store(STORE_FILE) {
                        let asked = store
                            .get(STORE_KEY_AUTOSTART_ASKED)
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if !asked {
                            store.set(STORE_KEY_AUTOSTART_ASKED, json!(true));
                            let _ = store.save();

                            open_autostart_prompt(app.handle());
                        }
                    }
                }
            }

            let update_handle = app.handle().clone();
            let update_strings = strings.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(UPDATE_CHECK_DELAY).await;
                updater::check_for_updates(update_handle, update_strings, false).await;
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Scoped to the main window on purpose: the update window is a real
            // window too, and closing it must close it — not hide the whole app
            // to the tray.
            if window.label() != MAIN_WINDOW {
                return;
            }

            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    let app = window.app_handle();
                    persist_session(app);

                    if let Some(strings) = app.try_state::<Strings>() {
                        maybe_show_tray_hint(app, &strings);
                    }

                    // Hide to tray instead of quitting so the WebSocket stays
                    // connected and OS toasts keep firing for incoming
                    // notifications. True exit is via the tray menu's Quit item.
                    let _ = window.hide();
                    api.prevent_close();
                }
                // Fires when the window is dragged onto a monitor with a
                // different scale factor, which is the other way geometry ends
                // up off-screen.
                WindowEvent::ScaleFactorChanged { .. } => {
                    if let Some(window) = window.app_handle().get_webview_window(MAIN_WINDOW) {
                        clamp_window_to_work_area(&window);
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running orcaa desktop");
}

/// Names the window after the page it is showing, the way every other desktop
/// app does. Falls back to the product name so the title is never empty or
/// stuck on a stale route.
fn set_window_title(window: &WebviewWindow, product: &str) {
    let window = window.clone();
    let product = product.to_string();

    let _ = window
        .clone()
        .eval_with_callback("document.title", move |value: String| {
            let page = value.trim().trim_matches('"').trim();
            let title = if page.is_empty() || page.eq_ignore_ascii_case(&product) {
                product.clone()
            } else {
                format!("{page} — {product}")
            };
            let _ = window.set_title(&title);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_above_the_top_of_the_screen_is_pulled_back_down() {
        // The reported bug: the title bar sliced in half by the top edge.
        let area = (0, 0, 1920, 1040);
        let (x, y, w, h) = clamped_rect((300, -18, 1440, 900), area);

        assert_eq!(
            (x, y),
            (300, 0),
            "the window must start at the work area top"
        );
        assert_eq!(
            (w, h),
            (1440, 900),
            "a window that already fits must not be resized"
        );
    }

    #[test]
    fn a_window_already_inside_the_work_area_is_left_alone() {
        let area = (0, 0, 1920, 1040);
        let rect = (120, 60, 1440, 900);

        assert_eq!(clamped_rect(rect, area), rect);
    }

    #[test]
    fn a_window_larger_than_the_work_area_is_shrunk_to_fit() {
        // 1440x900 is bigger than the 1366x768 panels a lot of reception desks
        // run, once the taskbar is taken off.
        let area = (0, 0, 1366, 728);
        let (x, y, w, h) = clamped_rect((0, 0, 1440, 900), area);

        assert_eq!((w, h), (1366, 728));
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn a_window_off_the_far_edges_is_pulled_fully_into_view() {
        let area = (0, 0, 1920, 1040);
        let (x, y, w, h) = clamped_rect((1900, 1030, 1440, 900), area);

        assert_eq!((x, y), (1920 - 1440, 1040 - 900));
        assert_eq!((w, h), (1440, 900));
    }

    #[test]
    fn a_secondary_monitor_at_a_negative_offset_is_respected() {
        // A monitor to the left of the primary has a negative origin; clamping
        // against 0 would teleport the window to the wrong screen.
        let area = (-1920, -200, 1920, 1040);
        let (x, y, _, _) = clamped_rect((-2100, -400, 1280, 800), area);

        assert_eq!((x, y), (-1920, -200));
    }

    #[test]
    fn the_taskbar_strip_is_respected() {
        // Work area, not monitor bounds: a window pinned to y=0 of a monitor
        // whose work area starts at y=40 (top-docked taskbar) must move down.
        let area = (0, 40, 1920, 1000);
        let (_, y, _, _) = clamped_rect((0, 0, 1200, 800), area);

        assert_eq!(y, 40);
    }

    #[test]
    fn the_shell_pages_are_never_stored_as_the_last_visited_url() {
        // Storing one reopened the app on its own scaffolding instead of the
        // page the user was actually working on.
        for raw in [
            "http://orcaa-shell.localhost/",
            "http://orcaa-shell.localhost/?waiting=1",
            "http://orcaa-shell.localhost/?update=1",
            "http://orcaa-shell.localhost/?loading=https%3A%2F%2Fclinic.orcaa.cloud%2F",
            "orcaa-shell://localhost/",
        ] {
            let url: Url = raw.parse().unwrap();
            assert!(
                is_shell_url(&url),
                "{raw} should be recognised as the shell"
            );
            assert!(!is_orcaa_host(&url), "{raw} must not be persisted");
            // Still internal — it must never be handed to the system browser.
            assert!(is_internal_url(&url));
        }
    }

    #[test]
    fn real_tenant_pages_are_still_persisted() {
        let url: Url = "https://clinic.orcaa.cloud/dashboard".parse().unwrap();
        assert!(is_orcaa_host(&url));
        assert!(!is_auth_host(&url));
    }

    #[test]
    fn sign_in_routes_are_bounced_but_handoffs_are_not() {
        let bounced = [
            "https://auth.orcaa.cloud/login",
            "https://clinic.orcaa.cloud/login",
            "https://clinic.orcaa.cloud/reset-password?token=x",
        ];
        for raw in bounced {
            assert!(
                is_sign_in_route(&raw.parse().unwrap()),
                "{raw} should bounce"
            );
        }

        let allowed = [
            "https://clinic.orcaa.cloud/desktop-handoff?token=x",
            "https://admin.orcaa.cloud/admin-handoff?token=x",
            "https://clinic.orcaa.cloud/dashboard",
        ];
        for raw in allowed {
            assert!(
                !is_sign_in_route(&raw.parse().unwrap()),
                "{raw} should load"
            );
        }
    }
}
