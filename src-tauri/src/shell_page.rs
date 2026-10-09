//! Every page the shell draws itself, plus the script it injects into the
//! remote app.
//!
//! The wrapper bundles no frontend assets (`frontendDist` is null — it only
//! renders the remote PWA), so its local pages are served from a registered URI
//! scheme instead.
//!
//! They are emphatically **not** `data:` URLs: Chromium blocks top-level
//! navigation to those, so WebView2 renders its own "can't reach this page"
//! error instead of the markup. Tauri's `webview-data-url` feature only
//! silences Tauri's own check — the engine still refuses.
//!
//! All four pages share one stylesheet and one document shell, so the shell has
//! exactly one place where brand tokens live. Never give a page its own colours.

use url::Url;

use crate::i18n::Strings;

pub const SHELL_SCHEME: &str = "orcaa-shell";

/// The real Orcaa mark, extracted from `shared/components/layout/logo/LogoSVG`
/// so the shell shows the actual brand rather than a stand-in. Inlined at
/// compile time — the wrapper serves no static files.
const ORCAA_LOGO_SVG: &str = include_str!("../assets/orcaa-logo.svg");

/// Which of the shell's own pages a URL refers to.
///
/// Modelled as an enum rather than loose query strings because every one of
/// these is reachable from several places (launch, sign-out, deep-link return,
/// tray) and a typo in a query key would silently fall through to the welcome
/// page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellPage {
    /// Entry state: the sign-in call to action.
    Welcome,
    /// The browser has been handed control; waiting for the deep link back.
    Waiting,
    /// Boot state. Probes `target` and hands off to it, or offers a retry.
    Loading { target: String },
    /// The branded update prompt. Carries no data — the page reads the pending
    /// update out of managed state when the request is served, so a stale URL
    /// can never describe a version that is no longer on offer.
    Update,
    /// The first-run "start with your computer?" offer — a branded window,
    /// never the OS's stock message box.
    AutostartPrompt,
}

impl ShellPage {
    fn query(&self) -> Option<String> {
        match self {
            ShellPage::Welcome => None,
            ShellPage::Waiting => Some("waiting=1".to_string()),
            ShellPage::Loading { target } => {
                let mut url = Url::parse("http://x/").expect("literal must parse");
                url.query_pairs_mut().append_pair("loading", target);
                url.query().map(str::to_string)
            }
            ShellPage::Update => Some("update=1".to_string()),
            ShellPage::AutostartPrompt => Some("autostart=1".to_string()),
        }
    }

    /// Parses the page back out of a served request's query string.
    pub fn from_query(query: &str) -> Self {
        // Parsed as real query pairs, not substring-matched: a target URL can
        // itself contain `waiting=` or `update=` in its own query.
        let url = Url::parse(&format!("http://x/?{query}")).ok();
        let pairs: Vec<(String, String)> = url
            .as_ref()
            .map(|u| {
                u.query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect()
            })
            .unwrap_or_default();

        let get = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.to_string())
        };

        if let Some(target) = get("loading") {
            return ShellPage::Loading { target };
        }
        if get("update").is_some() {
            return ShellPage::Update;
        }
        if get("autostart").is_some() {
            return ShellPage::AutostartPrompt;
        }
        if get("waiting").is_some() {
            return ShellPage::Waiting;
        }
        ShellPage::Welcome
    }
}

/// Windows serves custom schemes through `http://<scheme>.localhost`; the other
/// platforms use the scheme directly. Webview *creation* converts this for us,
/// but `navigate()` needs the already-converted form.
pub fn shell_url(page: &ShellPage, converted: bool) -> Url {
    let base = if converted && cfg!(windows) {
        format!("http://{SHELL_SCHEME}.localhost/")
    } else {
        format!("{SHELL_SCHEME}://localhost/")
    };

    let mut url: Url = base.parse().expect("shell URL must parse");
    url.set_query(page.query().as_deref());
    url
}

// ---------------------------------------------------------------------------
// Document shell
// ---------------------------------------------------------------------------

/// Values mirrored from `shared/styles/themes/{light,dark}/variables.css`.
/// They are copied, not invented: the shell renders before any app CSS exists,
/// so it cannot import the real tokens. Keep them in sync — and never retune the
/// brand here. Root font is 14px platform-wide, so 1rem is 14px, not 16px.
const SHELL_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --brand: #0891b2;
  --on-brand: #ffffff;
  --ground: #f8fafc;
  --card: #ffffff;
  --text: #1e293b;
  --text-soft: #64748b;
  --border: rgba(8, 145, 178, 0.2);
  --hairline: rgba(100, 116, 139, 0.18);
  --track: rgba(100, 116, 139, 0.16);
  --danger: #dc2626;
}
@media (prefers-color-scheme: dark) {
  :root {
    --brand: #00e0ff;
    --on-brand: #0a0a19;
    --ground: #0a0a19;
    --card: #140f2d;
    --text: rgba(255, 255, 255, 0.87);
    --text-soft: rgba(255, 255, 255, 0.6);
    --border: rgba(0, 224, 255, 0.2);
    --hairline: rgba(255, 255, 255, 0.12);
    --track: rgba(255, 255, 255, 0.14);
    --danger: #f87171;
  }
}
* { box-sizing: border-box; }
html { font-size: 14px; }
body {
  margin: 0; min-height: 100vh;
  background: var(--ground); color: var(--text);
  font-family: "Segoe UI", system-ui, -apple-system, "Helvetica Neue", sans-serif;
  -webkit-font-smoothing: antialiased;
  -webkit-user-select: none; user-select: none;
}
[hidden] { display: none !important; }

/* --- centred single-card pages (welcome, waiting, loading) --------------- */
body.centered { display: flex; align-items: center; justify-content: center; padding: 2rem; }
body.centered main {
  max-width: 30rem; width: 100%; text-align: center;
  background: var(--card); border: 1px solid var(--border);
  border-radius: 18px; padding: 3rem 2.5rem;
}
.mark { margin: 0 auto 1.75rem; width: 11rem; }
.mark svg { width: 100%; height: auto; display: block; }
h1 { font-size: 1.6rem; font-weight: 600; margin: 0 0 .75rem; line-height: 1.3; }
p { margin: 0 0 2rem; line-height: 1.7; font-size: 1rem; color: var(--text-soft); }

/* --- buttons ------------------------------------------------------------- */
.cta, button.primary, button.ghost {
  display: inline-block; padding: .85rem 2.25rem; border-radius: 12px;
  text-decoration: none; font-size: 1rem; font-weight: 600; font-family: inherit;
  border: 0; cursor: pointer; transition: filter .15s ease, background-color .15s ease;
}
.cta, button.primary { background: var(--brand); color: var(--on-brand); }
.cta:hover, button.primary:hover { filter: brightness(1.08); }
button.ghost {
  background: transparent; color: var(--text); border: 1px solid var(--hairline);
}
button.ghost:hover { background: var(--track); }
button.link {
  background: none; border: 0; padding: 0; cursor: pointer; font-family: inherit;
  font-size: .9rem; color: var(--text-soft); text-decoration: underline;
  text-underline-offset: 3px;
}
button.link:hover { color: var(--text); }
.cta:focus-visible, button:focus-visible { outline: 2px solid var(--brand); outline-offset: 3px; }

/* --- spinner ------------------------------------------------------------- */
.spinner {
  width: 1.75rem; height: 1.75rem; margin: 0 auto 1.5rem;
  border: 2px solid var(--track); border-top-color: var(--brand);
  border-radius: 50%; animation: spin .9s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }

/* --- update window ------------------------------------------------------- */
body.update { display: flex; flex-direction: column; height: 100vh; overflow: hidden;
  background: var(--card); border: 1px solid var(--border); border-radius: 12px; }
.titlebar {
  display: flex; align-items: center; gap: .5rem; flex: 0 0 auto;
  padding: .65rem .75rem .65rem 1rem; border-bottom: 1px solid var(--hairline);
}
.titlebar .mark { width: 5.25rem; margin: 0; }
.titlebar .close {
  margin-inline-start: auto; background: none; border: 0; cursor: pointer;
  color: var(--text-soft); border-radius: 8px; padding: .3rem;
  display: inline-flex; line-height: 0;
}
.titlebar .close:hover { background: var(--track); color: var(--text); }
body.update .body { flex: 1 1 auto; overflow-y: auto; padding: 1.5rem 1.5rem .5rem; }
body.update h1 { font-size: 1.25rem; margin: 0 0 .85rem; }
.versions { display: flex; align-items: center; gap: .5rem; margin-bottom: 1.25rem; }
.versions .v {
  font-variant-numeric: tabular-nums; font-size: .9rem; font-weight: 600;
  padding: .2rem .6rem; border-radius: 999px;
  background: var(--track); color: var(--text-soft);
}
.versions .v.next { background: var(--brand); color: var(--on-brand); }
.versions .arrow { color: var(--text-soft); }
.notes h2 {
  font-size: .78rem; font-weight: 600; text-transform: uppercase;
  letter-spacing: .06em; color: var(--text-soft); margin: 0 0 .5rem;
}
.notes-body {
  font-size: .92rem; line-height: 1.65; color: var(--text-soft);
  white-space: pre-wrap; max-height: 8rem; overflow-y: auto;
  -webkit-user-select: text; user-select: text;
}
.progress { margin-top: 1.25rem; }
.bar { height: 6px; border-radius: 999px; background: var(--track); overflow: hidden; }
.bar > i { display: block; height: 100%; width: 0; background: var(--brand);
  border-radius: 999px; transition: width .2s ease; }
.progress-meta {
  display: flex; justify-content: space-between; gap: 1rem; margin-top: .5rem;
  font-size: .82rem; color: var(--text-soft); font-variant-numeric: tabular-nums;
}
.error { margin-top: 1rem; font-size: .9rem; color: var(--danger); line-height: 1.6; }
body.update footer {
  flex: 0 0 auto; display: flex; align-items: center; gap: 1rem;
  padding: 1rem 1.5rem; border-top: 1px solid var(--hairline);
}
body.update footer .actions {
  margin-inline-start: auto; display: flex; gap: .6rem;
}
body.update footer button.primary, body.update footer button.ghost {
  padding: .6rem 1.15rem; font-size: .92rem; border-radius: 10px;
}

@media (prefers-reduced-motion: reduce) {
  .cta, button, .bar > i { transition: none; }
  .spinner { animation-duration: 2.4s; }
}
"#;

/// Suppresses the raw WebView2 menu on the shell's **own** pages only.
///
/// These have no content worth a context menu and no menu of their own, so
/// Edge's (Save as…, View source, Inspect) is pure noise. The remote app is
/// explicitly excluded — it ships its own right-click menu, and a listener from
/// out here would pre-empt it.
const NO_CONTEXT_MENU_JS: &str =
    "if(!__DEBUG__)document.addEventListener('contextmenu',e=>e.preventDefault());";

fn no_context_menu_js() -> String {
    NO_CONTEXT_MENU_JS.replace(
        "__DEBUG__",
        if cfg!(debug_assertions) {
            "true"
        } else {
            "false"
        },
    )
}

/// Wraps a body in the shared document shell. Every shell page goes through
/// here so `lang`/`dir` and the stylesheet can never drift apart between pages.
///
/// Note the shell pages carry no window chrome of their own — window topbar
/// and controls are owned and rendered by the frontend web app. The update
/// and autostart windows are separate webviews that keep their bespoke `.titlebar`.
fn document(strings: &Strings, title: &str, body_class: &str, body: &str, script: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="{lang}" dir="{dir}">
<meta charset="utf-8">
<title>{title}</title>
<style>{css}</style>
<body class="{body_class}">
{body}
</body>
<script>{no_menu}{script}</script>
</html>"#,
        lang = strings.html_lang(),
        dir = if strings.is_rtl() { "rtl" } else { "ltr" },
        title = escape_html(title),
        css = SHELL_CSS,
        body_class = body_class,
        body = body,
        no_menu = no_context_menu_js(),
        script = script,
    )
}

/// Anything interpolated into shell markup goes through here.
///
/// Release notes come off the update manifest and the loading target comes off
/// disk — neither is attacker-controlled today, but both are *data*, and data
/// that reaches an HTML document without escaping is how that stops being true.
fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A JSON string literal, safe to drop into a `<script>` block.
fn js_string(value: &str) -> String {
    // `serde_json` escapes the quotes and control characters; `</script` is the
    // one sequence it won't touch and the one that would break out of the tag.
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace("</", "<\\/")
}

// ---------------------------------------------------------------------------
// Sign-in pages
// ---------------------------------------------------------------------------

const SIGNIN_JS: &str = r#"
(() => {
  const cta = document.getElementById('signin');
  if (!cta) return;
  // A command, deliberately — not a link the shell intercepts.
  //
  // This used to be an anchor pointing at a magic "?action=signin" URL that
  // `on_navigation` watched for. That made *any* navigation to that URL open the
  // system browser, so landing on this page could hijack the browser without
  // anybody clicking anything. An invoke can only originate from this handler,
  // which only runs on a real click.
  cta.addEventListener('click', () => {
    cta.disabled = true;
    window.__TAURI_INTERNALS__.invoke('signin_start', {});
  });
})();
"#;

/// The shell's sign-in page.
///
/// Two states, one document: an entry state with the primary call to action,
/// and a waiting state once the browser has been handed control. Landing here
/// is *never* enough to open the browser — see [`SIGNIN_JS`].
pub fn holding_page_html(strings: &Strings, waiting: bool) -> String {
    let (title, body, cta) = if waiting {
        (
            strings.signin_title(),
            strings.signin_body(),
            strings.signin_retry(),
        )
    } else {
        (
            strings.signin_welcome_title(),
            strings.signin_welcome_body(),
            strings.signin_cta(),
        )
    };

    let markup = format!(
        r#"<main>
  <div class="mark" role="img" aria-label="Orcaa">{logo}</div>
  <h1>{title}</h1>
  <p>{body}</p>
  <button class="cta" id="signin" type="button">{cta}</button>
</main>"#,
        logo = ORCAA_LOGO_SVG,
        title = escape_html(&title),
        body = escape_html(&body),
        cta = escape_html(&cta),
    );

    document(strings, &title, "centered", &markup, SIGNIN_JS)
}

// ---------------------------------------------------------------------------
// Loading / offline page
// ---------------------------------------------------------------------------

const LOADING_JS: &str = r#"
(() => {
  const target = __TARGET__;
  const loading = document.getElementById('loading');
  const offline = document.getElementById('offline');
  const retry = document.getElementById('retry');

  // Probes reachability through a HIDDEN IFRAME rather than a bare `fetch`.
  //
  // The old version fetched the target with `no-cors` from THIS page's own
  // `orcaa-shell://` origin — a cross-origin network request that can never
  // be answered by the target tenant's own service worker (a SW only
  // intercepts fetches made by documents of its OWN origin, never a probe
  // from somewhere else). That made the probe equivalent to "is the real
  // network up", which is exactly wrong once the target origin can serve a
  // cached app shell offline (see the frontend's `sw.js` navigation
  // handler): a device that had visited before, went offline, and could
  // perfectly well boot from cache was told it couldn't, before ever
  // getting the chance to try.
  //
  // An iframe NAVIGATION to the target IS a same-origin document load once
  // it lands — `request.mode` is `navigate` there exactly like a top-level
  // load — so the target's own SW gets first crack at it, cached shell
  // included. `onload` firing (even for a SW-served fallback page) means
  // the real top-level navigation will succeed the same way; `onerror` or
  // the timeout elapsing means neither the network nor any cache could
  // answer, which is the genuine "nothing to boot from" case.
  let probeFrame = null;
  let settled = false;

  const cleanupFrame = () => {
    if (probeFrame) { probeFrame.remove(); probeFrame = null; }
  };

  const goToTarget = () => {
    cleanupFrame();
    // `replace` keeps the boot page out of history.
    window.location.replace(target);
  };

  const probe = () => {
    loading.hidden = false;
    offline.hidden = true;
    settled = false;
    cleanupFrame();

    const timer = window.setTimeout(() => {
      if (settled) return;
      settled = true;
      showOffline();
    }, 6000);

    probeFrame = document.createElement('iframe');
    probeFrame.style.display = 'none';
    probeFrame.addEventListener('load', () => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      goToTarget();
    });
    probeFrame.addEventListener('error', () => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      showOffline();
    });
    probeFrame.src = new URL('/', target).toString();
    document.body.appendChild(probeFrame);
  };

  const showOffline = () => {
    cleanupFrame();
    loading.hidden = true;
    offline.hidden = false;
    retry.focus();
  };

  retry.addEventListener('click', probe);
  // Coming back online mid-wait should just work, without a click.
  window.addEventListener('online', () => { if (!offline.hidden) probe(); });

  probe();
})();
"#;

/// The boot page.
///
/// The window used to be built hidden and revealed on first paint, which meant
/// a slow or dead network showed nothing at all for up to twelve seconds. This
/// page is shown instead — instantly, before any network work — and hands off to
/// the real app once it has confirmed the host answers. When it doesn't, the
/// same document becomes the retry surface, so an unreachable backend never
/// falls through to WebView2's own error page.
pub fn loading_page_html(strings: &Strings, target: &str) -> String {
    let markup = format!(
        r#"<main>
  <div class="mark" role="img" aria-label="Orcaa">{logo}</div>
  <div id="loading">
    <div class="spinner" role="status" aria-live="polite" aria-label="{loading_label}"></div>
    <h1>{loading_title}</h1>
    <p>{loading_body}</p>
  </div>
  <div id="offline" hidden>
    <h1>{offline_title}</h1>
    <p>{offline_body}</p>
    <button class="cta" id="retry" type="button">{retry}</button>
  </div>
</main>"#,
        logo = ORCAA_LOGO_SVG,
        loading_label = escape_html(&strings.loading_title()),
        loading_title = escape_html(&strings.loading_title()),
        loading_body = escape_html(&strings.loading_body()),
        offline_title = escape_html(&strings.offline_title()),
        offline_body = escape_html(&strings.offline_body()),
        retry = escape_html(&strings.offline_retry()),
    );

    document(
        strings,
        &strings.loading_title(),
        "centered",
        &markup,
        &LOADING_JS.replace("__TARGET__", &js_string(target)),
    )
}

// ---------------------------------------------------------------------------
// Update window
// ---------------------------------------------------------------------------

const UPDATE_JS: &str = r#"
(() => {
  const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {});
  const el = (id) => document.getElementById(id);
  const strings = __STRINGS__;

  const install = el('install');
  const later = el('later');
  const skip = el('skip');
  const close = el('close');
  const footer = el('footer');
  const progress = el('progress');
  const fill = el('fill');
  const pct = el('pct');
  const bytes = el('bytes');
  const status = el('status');
  const error = el('error');

  const mb = (n) => (n / 1048576).toFixed(1) + ' MB';

  later.addEventListener('click', () => invoke('update_snooze'));
  close.addEventListener('click', () => invoke('update_snooze'));
  skip.addEventListener('click', () => invoke('update_skip'));

  install.addEventListener('click', () => {
    // The footer is retired rather than disabled: once bytes are moving the
    // only honest option left is to wait, and a greyed-out row of dead buttons
    // reads as a broken window.
    footer.hidden = true;
    error.hidden = true;
    progress.hidden = false;
    status.textContent = strings.downloading;
    invoke('update_install');
  });

  // Rust drives these directly with `eval` rather than events, which keeps the
  // window off the event plugin's permission surface entirely.
  window.orcaaUpdate = {
    progress(done, total) {
      progress.hidden = false;
      if (total > 0) {
        const ratio = Math.min(1, done / total);
        fill.style.width = (ratio * 100).toFixed(1) + '%';
        pct.textContent = Math.round(ratio * 100) + '%';
        bytes.textContent = mb(done) + ' / ' + mb(total);
      } else {
        // A manifest without a content length still deserves a live figure.
        pct.textContent = '';
        bytes.textContent = mb(done);
      }
    },
    installing() {
      fill.style.width = '100%';
      pct.textContent = '100%';
      status.textContent = strings.installing;
    },
    failed(message) {
      progress.hidden = true;
      footer.hidden = false;
      error.hidden = false;
      error.textContent = message || strings.failed;
    },
  };

  // Escape is the frameless-window equivalent of the close button.
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') invoke('update_snooze');
  });
})();
"#;

/// The branded update prompt.
///
/// Replaces a native `MessageDialogButtons::OkCancelCustom` message box, which
/// was unbranded, blocking, took its own taskbar entry (so it read as a second
/// app rather than part of this one), showed no download progress, and — because
/// Win32 falls back to a plain `MessageBox` when it can't raise a TaskDialog —
/// routinely rendered "Remind me later" as a bare `Cancel`.
pub fn update_page_html(
    strings: &Strings,
    current: &str,
    next: &str,
    notes: Option<&str>,
) -> String {
    let notes_body = notes
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(escape_html)
        .unwrap_or_else(|| escape_html(&strings.update_notes_fallback()));

    let strings_json = format!(
        r#"{{"downloading":{},"installing":{},"failed":{}}}"#,
        js_string(&strings.update_progress_downloading()),
        js_string(&strings.update_progress_installing()),
        js_string(&strings.update_download_failed()),
    );

    let markup = format!(
        r#"<div class="titlebar" data-tauri-drag-region>
  <div class="mark" role="img" aria-label="Orcaa" data-tauri-drag-region>{logo}</div>
  <button class="close" id="close" type="button" title="{later}" aria-label="{later}">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
  </button>
</div>
<div class="body">
  <h1>{heading}</h1>
  <div class="versions">
    <span class="v">{current}</span>
    <span class="arrow" aria-hidden="true">{arrow}</span>
    <span class="v next">{next}</span>
  </div>
  <section class="notes">
    <h2>{notes_heading}</h2>
    <div class="notes-body">{notes_body}</div>
  </section>
  <div class="progress" id="progress" hidden>
    <div class="bar"><i id="fill"></i></div>
    <div class="progress-meta"><span id="pct">0%</span><span id="bytes"></span></div>
    <p id="status" style="margin:.6rem 0 0;font-size:.85rem"></p>
  </div>
  <div class="error" id="error" role="alert" hidden></div>
</div>
<footer id="footer">
  <button class="link" id="skip" type="button">{skip}</button>
  <div class="actions">
    <button class="ghost" id="later" type="button">{later}</button>
    <button class="primary" id="install" type="button">{install}</button>
  </div>
</footer>"#,
        logo = ORCAA_LOGO_SVG,
        heading = escape_html(&strings.update_prompt_title()),
        current = escape_html(current),
        next = escape_html(next),
        arrow = if strings.is_rtl() { "&larr;" } else { "&rarr;" },
        notes_heading = escape_html(&strings.update_notes_heading()),
        notes_body = notes_body,
        skip = escape_html(&strings.update_skip_version()),
        later = escape_html(&strings.update_remind_later()),
        install = escape_html(&strings.update_install_now()),
    );

    document(
        strings,
        &strings.update_prompt_title(),
        "update",
        &markup,
        &UPDATE_JS.replace("__STRINGS__", &strings_json),
    )
}

// ---------------------------------------------------------------------------
// Autostart offer window
// ---------------------------------------------------------------------------

const AUTOSTART_JS: &str = r#"
(() => {
  const invoke = (cmd) => window.__TAURI_INTERNALS__.invoke(cmd, {});
  document.getElementById('accept').addEventListener('click', () => invoke('autostart_accept'));
  const decline = () => invoke('autostart_decline');
  document.getElementById('decline').addEventListener('click', decline);
  document.getElementById('close').addEventListener('click', decline);
})();
"#;

/// The first-run "start with your computer?" offer.
///
/// Wears the SAME branded chrome as the update prompt (`body.update`: logo
/// titlebar, body, footer actions) — this used to be the OS's stock message
/// box, which asked the right question in the wrong clothes: an unbranded
/// system dialog floating over an otherwise fully Orcaa surface.
pub fn autostart_prompt_page_html(strings: &Strings) -> String {
    let markup = format!(
        r#"<div class="titlebar" data-tauri-drag-region>
  <div class="mark" role="img" aria-label="Orcaa" data-tauri-drag-region>{logo}</div>
  <button class="close" id="close" type="button" title="{not_now}" aria-label="{not_now}">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
  </button>
</div>
<div class="body">
  <h1>{title}</h1>
  <p>{body}</p>
</div>
<footer>
  <div class="actions">
    <button class="ghost" id="decline" type="button">{not_now}</button>
    <button class="primary" id="accept" type="button">{accept}</button>
  </div>
</footer>"#,
        logo = ORCAA_LOGO_SVG,
        title = escape_html(&strings.autostart_prompt_title()),
        body = escape_html(&strings.autostart_prompt_body()),
        accept = escape_html(&strings.tray_autostart()),
        not_now = escape_html(&strings.autostart_prompt_not_now()),
    );

    document(
        strings,
        &strings.autostart_prompt_title(),
        "update",
        &markup,
        AUTOSTART_JS,
    )
}

// ---------------------------------------------------------------------------
// Script injected into the remote app
// ---------------------------------------------------------------------------

const INIT_JS: &str = r#"
(() => {
  const DEBUG = __DEBUG__;

  // --- shell facts the web app may branch on ------------------------------
  // Set synchronously at document-start, before a line of the app runs, so
  // `isLegacyShell()` is a plain property read — no command round-trip, no
  // capability to grant. `legacy` is true only in the Windows 7 build, where
  // WebView2 is pinned at 109 and the OS has no toast centre; the app uses it
  // to skip the notification plugin (not compiled in there) and to show its
  // one-time compatibility notice. `webview` is the runtime version string.
  //
  // `machine` is the salted hash of this computer's OS machine id (see
  // machine.rs), or "" when the OS would not say. The app sends it as
  // `X-Hardware-Id` so a platform ban can be keyed on the PC itself — the one
  // identifier that survives clearing storage. Shells before 1.4.6 publish no
  // `machine` at all, which the app reads the same way as "".
  window.__ORCAA_SHELL__ = Object.freeze({ legacy: __LEGACY__, webview: __WEBVIEW__, machine: __MACHINE__ });

  const invoke = (cmd, args) => {
    try { return window.__TAURI_INTERNALS__.invoke(cmd, args || {}); }
    catch (_) { return Promise.resolve(); }
  };

  // --- context menu ------------------------------------------------------
  // Deliberately NOT handled here. The app ships its own right-click menu
  // (`useGlobalContextMenu`), which already suppresses the raw WebView2 one
  // everywhere it draws its own, and deliberately leaves the native menu on
  // editable fields so paste and spellcheck keep working.
  //
  // A `contextmenu` listener in this script is injected at document-start, so it
  // runs BEFORE the app's. Calling `preventDefault()` here made the app's own
  // handler see `e.defaultPrevented` and bail — right-clicking the page gave no
  // menu at all. The shell's own pages suppress it locally instead.

  // --- keyboard ----------------------------------------------------------
  // WebView2 handles reload and history natively on Windows; webkitgtk does
  // not. Routing them through commands keeps the behaviour identical on all
  // three platforms. Zoom combinations are swallowed outright: the UI is
  // designed for a fixed 100% zoom (the app's font-scale preference is the
  // sanctioned knob), so the shell pins the webview at 1.0 and these keys do
  // nothing.
  //
  // Only these exact combinations are claimed. The app owns bare keys — its
  // own "/" search shortcut must keep working.
  window.addEventListener('keydown', (e) => {
    const mod = e.ctrlKey || e.metaKey;
    const plain = !e.altKey && !e.shiftKey;

    if (mod && plain && (e.key === '+' || e.key === '=' || e.code === 'NumpadAdd')) {
      e.preventDefault(); return;
    }
    if (mod && plain && (e.key === '-' || e.key === '_' || e.code === 'NumpadSubtract')) {
      e.preventDefault(); return;
    }
    if (mod && plain && (e.key === '0' || e.code === 'Numpad0')) {
      e.preventDefault(); return;
    }
    if ((mod && e.key.toLowerCase() === 'r') || e.key === 'F5') {
      e.preventDefault();
      // The app's rule is "re-pull this view's data, never reload the app" —
      // a full reload throws away React state, scroll position and every warm
      // query for what the user meant as "refresh". Ask the app first; a
      // cancelled event is its "I handled it". Nothing listening (the shell's
      // own pages, or a hard reload) falls through to the real thing.
      const hard = e.shiftKey;
      const handled =
        !hard &&
        !window.dispatchEvent(
          new CustomEvent('orcaa:refresh', { cancelable: true }),
        );
      if (!handled) invoke('shell_reload');
      return;
    }
    if (e.altKey && !mod && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      // Deliberately NOT mirrored for Arabic. The page direction is not the
      // shortcut's frame of reference — Windows binds Alt+Left to "back"
      // whatever the content says, and an app that swapped them under an RTL
      // locale would be the only one on the machine doing so.
      e.preventDefault(); history.go(e.key === 'ArrowLeft' ? -1 : 1); return;
    }
    if (e.key === 'F11') {
      e.preventDefault(); invoke('shell_fullscreen_toggle'); return;
    }
    if (mod && plain && e.key.toLowerCase() === 'q') {
      e.preventDefault(); invoke('shell_quit'); return;
    }
  });

  // Ctrl+wheel (and precision-touchpad pinch, which arrives as a synthetic
  // ctrlKey wheel) would otherwise zoom the webview — swallow it so the page
  // stays at 100%.
  window.addEventListener('wheel', (e) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
  }, { passive: false });
})();
"#;

/// The script injected into every page the main window loads, including the
/// remote app. `webview_version` is the runtime's version string when Tauri
/// could read it (`tauri::webview_version()`); it is exposed to the app as a
/// string literal, so it goes through the same escaping as every other value
/// that lands in a script.
///
/// `machine` is `machine::machine_hash()` — a hex digest we produced, but it is
/// still escaped like any other value rather than trusted to be inert.
pub fn shell_init_js(
    _strings: &Strings,
    webview_version: Option<&str>,
    machine: Option<&str>,
) -> String {
    INIT_JS
        .replace(
            "__DEBUG__",
            if cfg!(debug_assertions) {
                "true"
            } else {
                "false"
            },
        )
        .replace(
            "__LEGACY__",
            if cfg!(legacy_win7) { "true" } else { "false" },
        )
        .replace("__WEBVIEW__", &js_string(webview_version.unwrap_or("")))
        .replace("__MACHINE__", &js_string(machine.unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings() -> Strings {
        Strings::detect("Orcaa".to_string())
    }

    #[test]
    fn landing_on_the_sign_in_page_cannot_open_the_browser_by_itself() {
        for waiting in [false, true] {
            let html = holding_page_html(&strings(), waiting);

            // Pressing the button is the ONLY thing that may open the browser.
            // The page must therefore contain no navigable route to sign-in at
            // all: no link to the auth host, and no magic URL for the shell to
            // intercept. Merely *landing* here — which is what a sign-out, an
            // expired session and a cold launch all do — must be inert.
            // No anchors at all, so there is nothing on the page that a stray
            // navigation could follow. (The logo's internal `href="#clipRect"`
            // is an SVG clip-path reference, not a link.)
            assert!(
                !html.contains("<a ") && !html.contains("<a>"),
                "the sign-in page must contain no links at all"
            );
            assert!(!html.contains("auth."), "no reference to the auth host");
            assert!(
                html.contains("invoke('signin_start'"),
                "sign-in must be a command fired from a click handler"
            );
            assert!(html.contains("<h1>"));
        }
    }

    #[test]
    fn pages_round_trip_through_their_query_strings() {
        let cases = [
            ShellPage::Welcome,
            ShellPage::Waiting,
            ShellPage::Update,
            ShellPage::AutostartPrompt,
            ShellPage::Loading {
                target: "https://clinic.orcaa.cloud/dashboard?tab=overview".to_string(),
            },
        ];

        for page in cases {
            let url = shell_url(&page, true);
            let parsed = ShellPage::from_query(url.query().unwrap_or_default());
            assert_eq!(parsed, page, "{url} should round-trip");
        }
    }

    #[test]
    fn a_loading_target_carrying_its_own_query_keys_is_not_misread() {
        // Substring matching on "waiting=" or "update=" would have classified
        // this as the wrong page and dropped the user on a dead screen.
        let page = ShellPage::Loading {
            target: "https://clinic.orcaa.cloud/x?waiting=1&update=1".to_string(),
        };
        let url = shell_url(&page, true);

        assert_eq!(ShellPage::from_query(url.query().unwrap()), page);
    }

    #[test]
    fn the_shell_does_not_inject_window_chrome() {
        // Window chrome / topbar is owned and drawn entirely by the frontend web apps
        // (AppTopbar + WindowControls). The desktop shell must not inject a duplicate
        // titlebar into the webview.
        let js = shell_init_js(&strings(), None, None);
        assert!(!js.contains("orcaa-shell-titlebar"));
        assert!(!js.contains("drag-edge"));

        let welcome = holding_page_html(&strings(), false);
        assert!(!welcome.contains("data-tauri-drag-region"));
        assert!(!welcome.contains("shell_window_control"));
    }

    #[test]
    fn the_autostart_offer_is_a_branded_page_with_both_choices() {
        // This used to be the OS's stock message box — a regression here means
        // an unbranded system dialog is back in front of users.
        let html = autostart_prompt_page_html(&strings());

        assert!(html.contains("autostart_accept"));
        assert!(html.contains("autostart_decline"));
        assert!(
            html.contains("data-tauri-drag-region"),
            "frameless window must drag"
        );
        assert!(
            html.contains("class=\"update\""),
            "must wear the branded window chrome"
        );
        assert!(
            html.contains("<svg"),
            "must carry the Orcaa mark, not a stock icon"
        );
    }

    #[test]
    fn interpolated_data_cannot_break_out_of_the_markup() {
        let html = update_page_html(
            &strings(),
            "1.0.0",
            "1.1.6",
            Some("<img src=x onerror=alert(1)>\n</script><b>hi</b>"),
        );

        assert!(!html.contains("<img src=x"), "notes must be escaped");
        assert!(html.contains("&lt;img src=x"));
        // The only </script> in the document is the one closing our own block.
        assert_eq!(html.matches("</script>").count(), 1);
    }

    #[test]
    fn the_loading_page_probes_reachability_through_an_iframe_not_a_cross_origin_fetch() {
        // A `fetch(target, {mode: 'no-cors'})` from THIS page's own
        // `orcaa-shell://` origin can never be answered by the target
        // tenant's service worker (a SW only intercepts same-origin
        // document/fetch traffic) — so it tested real network reachability
        // only, which is exactly wrong once the target origin can serve a
        // cached shell offline. The probe must go through an iframe
        // NAVIGATION instead, which the target's own SW gets a chance to
        // answer the same way a real top-level navigation would.
        let html = loading_page_html(&strings(), "https://clinic.orcaa.cloud/");

        assert!(
            !html.contains("mode: 'no-cors'") && !html.contains("mode:'no-cors'"),
            "must not probe via a cross-origin no-cors fetch"
        );
        assert!(
            html.contains("createElement('iframe')"),
            "must probe reachability through an iframe navigation"
        );
        assert!(
            html.contains("addEventListener('load'") && html.contains("addEventListener('error'"),
            "must resolve the probe from the iframe's own load/error events"
        );
    }

    #[test]
    fn the_loading_page_does_not_skip_the_probe_just_because_navigator_online_is_false() {
        // navigator.onLine is a coarse OS-level signal that can be wrong in
        // either direction; short-circuiting straight to the offline page
        // on `navigator.onLine === false` denied a device that could
        // perfectly well boot from its own cached shell the chance to try.
        let html = loading_page_html(&strings(), "https://clinic.orcaa.cloud/");

        assert!(
            !html.contains("navigator.onLine"),
            "must always attempt the probe rather than trusting navigator.onLine alone"
        );
    }

    #[test]
    fn the_loading_target_is_injected_as_a_js_string_not_as_code() {
        let html = loading_page_html(&strings(), "https://clinic.orcaa.cloud/\"+alert(1)+\"");

        assert!(html.contains(r#"\"+alert(1)+\""#), "quotes must be escaped");
        assert!(!html.contains(r#"= "https://clinic.orcaa.cloud/"+alert(1)+"";"#));
    }

    #[test]
    fn the_update_page_offers_all_three_choices() {
        let html = update_page_html(&strings(), "1.1.6", "1.0.20", None);

        for id in [
            "id=\"install\"",
            "id=\"later\"",
            "id=\"skip\"",
            "id=\"close\"",
        ] {
            assert!(html.contains(id), "the update window must render {id}");
        }
        assert!(
            html.contains("data-tauri-drag-region"),
            "frameless window must be draggable"
        );
        assert!(html.contains("1.1.6") && html.contains("1.0.20"));
    }

    #[test]
    fn the_injected_script_only_claims_modified_keys() {
        let js = shell_init_js(&strings(), None, None);

        // A bare-key binding here would shadow the app's own "/" search.
        assert!(js.contains("e.ctrlKey || e.metaKey"));
        // Zoom is pinned at 100% — the zoom combinations are swallowed, never
        // routed to a command.
        assert!(!js.contains("shell_zoom"), "zoom must stay locked at 100%");
        assert!(
            !js.contains("__DEBUG__"),
            "the debug flag must be substituted"
        );
    }

    #[test]
    fn the_injected_script_publishes_shell_facts_before_anything_else() {
        // `isLegacyShell()` in the web app is a synchronous property read at
        // module-evaluation time, so the global must be set before the first
        // command could possibly be invoked — and both placeholders must be
        // substituted, or the page would throw a ReferenceError at startup.
        let js = shell_init_js(&strings(), Some("109.0.1518.140"), None);

        let global = js
            .find("window.__ORCAA_SHELL__")
            .expect("the shell facts global must be published");
        let first_invoke = js.find("invoke(").expect("the script invokes commands");
        assert!(global < first_invoke, "shell facts must come first");

        assert!(!js.contains("__LEGACY__") && !js.contains("__WEBVIEW__"));
        // An unsubstituted `__MACHINE__` would be a ReferenceError on the very
        // first line of every page — the app would not start.
        assert!(!js.contains("__MACHINE__"));
        assert!(js.contains(r#"machine: """#), "no machine id reads as empty");
        assert!(js.contains(r#"webview: "109.0.1518.140""#));
        assert!(
            js.contains(if cfg!(legacy_win7) {
                "legacy: true"
            } else {
                "legacy: false"
            }),
            "legacy must mirror the build's cfg"
        );

        // No version known → an empty string, never `undefined` or a bare hole.
        let js = shell_init_js(&strings(), None, None);
        assert!(js.contains(r#"webview: """#));
    }

    #[test]
    fn the_machine_hash_is_published_as_an_escaped_string() {
        let machine = "9f".repeat(32);
        let js = shell_init_js(&strings(), None, Some(&machine));

        assert!(js.contains(&format!(r#"machine: "{machine}""#)));

        // It is our own hex digest, but it still goes through the same
        // escaping as everything else that lands in a script: a value that
        // could close the string would be script injection into every page.
        let hostile = shell_init_js(&strings(), None, Some(r#""});alert(1);({x:""#));
        assert!(!hostile.contains(r#"machine: ""});alert(1)"#));
    }

    #[test]
    fn the_injected_script_never_touches_the_apps_context_menu() {
        // Regression guard. This script is injected at document-start, so a
        // `contextmenu` listener here runs BEFORE the app's `useGlobalContextMenu`
        // — and the moment it calls `preventDefault()` the app's handler sees
        // `defaultPrevented` and bails, leaving the page with no menu at all.
        let js = shell_init_js(&strings(), None, None);

        assert!(
            !js.contains("addEventListener('contextmenu'"),
            "the remote app owns its own right-click menu"
        );
    }

    #[test]
    fn the_shells_own_pages_do_suppress_the_native_menu() {
        // ...whereas the shell's pages have no menu of their own, so Edge's
        // (Save as…, View source) is pure noise there.
        let html = holding_page_html(&strings(), false);

        assert!(html.contains("contextmenu"));
    }

    #[test]
    fn refresh_asks_the_app_before_reloading_the_webview() {
        // A full reload throws away React state and every warm query for what
        // the user meant as "refresh"; the app re-pulls data instead. The hard
        // reload stays available on Ctrl+Shift+R.
        let js = shell_init_js(&strings(), None, None);

        assert!(
            js.contains("orcaa:refresh"),
            "must offer the app the event first"
        );
        assert!(
            js.contains("cancelable: true"),
            "the app signals it handled it by cancelling"
        );
        assert!(
            js.contains("shell_reload"),
            "and fall back to a real reload"
        );
    }
}
