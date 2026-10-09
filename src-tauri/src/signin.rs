//! Browser-based sign-in.
//!
//! Auth never runs inside the webview. Google refuses OAuth in embedded
//! webviews (`disallowed_useragent`), password managers don't reach in there,
//! and an app that renders its own login page is asking users to type
//! credentials into a window they can't verify. So the shell hands sign-in to
//! the user's real browser and waits for the result to come back over the
//! `orcaa://` deep link.
//!
//! ## The exchange, and why it is shaped this way
//!
//! ```text
//!   shell                        system browser                backend
//!     │  verifier (kept)
//!     │  challenge = sha256 ─────────►  auth.orcaa.cloud
//!     │                                  │ user signs in
//!     │                                  ├──── issue(challenge) ────►
//!     │                                  ◄──── ticket ──────────────┤
//!     ◄──── orcaa://auth?token=…&state=…─┘
//!     │
//!     └─ webview → /desktop-handoff?token=…&verifier=…  ──── exchange ───►
//!                                                       ◄─── JWTs ────────┘
//! ```
//!
//! Any locally-installed program can register the `orcaa://` scheme, so the
//! deep link must be assumed readable by an attacker. Two things defend it:
//!
//! - **The verifier never leaves this process.** Only its SHA-256 goes through
//!   the browser, so a captured ticket cannot be redeemed (backend enforces).
//! - **`state` is checked on return.** A deep link the shell didn't initiate is
//!   dropped, so nothing can push an attacker-chosen session into the window.

use std::sync::Mutex;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};
use url::Url;

/// The in-flight sign-in attempt. `None` whenever the shell isn't expecting a
/// callback — which is exactly what makes an unsolicited deep link a no-op.
#[derive(Default)]
pub struct PendingSignIn(Mutex<Option<Attempt>>);

#[derive(Clone)]
pub struct Attempt {
    state: String,
    verifier: String,
}

/// Where a completed sign-in should drop the user.
pub struct Resolved {
    pub url: Url,
}

impl PendingSignIn {
    /// Starts an attempt and returns the browser URL to open.
    ///
    /// Replaces any previous attempt: if someone restarts sign-in, only the
    /// newest callback should be honoured (the backend likewise deletes the
    /// older ticket).
    ///
    /// `machine` is this computer's hashed id (`machine::machine_hash`), when
    /// the OS gave one. It rides along as `dh` so the auth app can label its
    /// requests with the PC the sign-in started on. It is NOT part of the
    /// exchange's security — nothing checks it on the way back, and a link
    /// with it stripped is simply an ordinary browser sign-in.
    pub fn begin(&self, auth_base: &str, scheme: &str, machine: Option<&str>) -> Option<Url> {
        let attempt = Attempt {
            state: random_token(32),
            verifier: random_token(64),
        };

        let mut url = Url::parse(auth_base).ok()?;
        url.set_path("/login");
        url.query_pairs_mut()
            .append_pair("desktop", "1")
            .append_pair("ds", &attempt.state)
            .append_pair("dc", &challenge_for(&attempt.verifier))
            // Which scheme to call back on. Both desktop builds share this
            // codebase but must NOT share a scheme: whichever installed last
            // would win the registration and swallow the other's callbacks.
            .append_pair("dsch", scheme);

        if let Some(machine) = machine {
            url.query_pairs_mut().append_pair("dh", machine);
        }

        *self.0.lock().ok()? = Some(attempt);

        Some(url)
    }

    /// Validates a returning deep link and produces the in-app URL to load.
    ///
    /// Returns `None` — silently — for anything unexpected: a link that arrived
    /// without a pending attempt, a mismatched `state`, a missing ticket, or a
    /// subdomain that isn't a plain label. Callers must not surface the
    /// difference; a hostile link should look exactly like a stale one.
    pub fn resolve(&self, incoming: &Url, base_domain: &str, scheme: &str) -> Option<Resolved> {
        if incoming.scheme() != scheme {
            return None;
        }

        let params: Vec<(String, String)> = incoming
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.to_string())
        };

        let state = get("state")?;
        let token = get("token")?;
        let subdomain = get("subdomain")?;

        // Take the attempt regardless of outcome — a single ticket gets a
        // single shot, so a failed match can't be retried by replaying links.
        let attempt = self.0.lock().ok()?.take()?;

        if !constant_time_eq(&attempt.state, &state) {
            return None;
        }

        if !is_plain_label(&subdomain) {
            return None;
        }

        let mut url = Url::parse(&format!("https://{subdomain}.{base_domain}")).ok()?;
        url.set_path("/desktop-handoff");
        url.query_pairs_mut()
            .append_pair("token", &token)
            .append_pair("verifier", &attempt.verifier);

        Some(Resolved { url })
    }
}

fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Must match `DesktopHandoffService::challengeFor()` on the backend exactly —
/// base64url of the raw SHA-256 digest, no padding.
fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Rejects anything that isn't a single DNS label, so a crafted `subdomain`
/// can't steer the webview off the tenant domain (`evil.com/#`, `a.b`, `../`).
fn is_plain_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin() -> (PendingSignIn, String) {
        let pending = PendingSignIn::default();
        let url = pending
            .begin("https://auth.orcaa.cloud", "orcaa", None)
            .unwrap();
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "ds")
            .unwrap()
            .1
            .into_owned();
        (pending, state)
    }

    #[test]
    fn browser_url_carries_the_challenge_not_the_verifier() {
        let pending = PendingSignIn::default();
        let url = pending
            .begin("https://auth.orcaa.cloud", "orcaa", None)
            .unwrap();
        let query = url.query().unwrap();

        assert!(query.contains("desktop=1"));
        assert!(query.contains("dc="));

        let verifier = pending.0.lock().unwrap().clone().unwrap().verifier;
        assert!(
            !query.contains(&verifier),
            "the verifier must never reach the browser"
        );
    }

    #[test]
    fn browser_url_carries_the_machine_hash_when_there_is_one() {
        let machine = "a".repeat(64);
        let pending = PendingSignIn::default();
        let url = pending
            .begin("https://auth.orcaa.cloud", "orcaa", Some(&machine))
            .unwrap();

        let carried = url
            .query_pairs()
            .find(|(k, _)| k == "dh")
            .map(|(_, v)| v.into_owned());
        assert_eq!(carried.as_deref(), Some(machine.as_str()));

        // Adding it must not have disturbed the exchange itself.
        let query = url.query().unwrap();
        let verifier = pending.0.lock().unwrap().clone().unwrap().verifier;
        assert!(query.contains("dc=") && query.contains("ds="));
        assert!(
            !query.contains(&verifier),
            "the verifier must never reach the browser"
        );
    }

    #[test]
    fn browser_url_has_no_machine_parameter_when_the_os_gave_none() {
        // No id is the ordinary browser case, not an error — and an empty
        // `dh=` would be a value the auth app has to special-case.
        let pending = PendingSignIn::default();
        let url = pending
            .begin("https://auth.orcaa.cloud", "orcaa", None)
            .unwrap();

        assert!(url.query_pairs().all(|(k, _)| k != "dh"));
    }

    #[test]
    fn challenge_matches_the_backend_derivation() {
        // Cross-checked against PHP:
        //   rtrim(strtr(base64_encode(hash('sha256', 'orcaa', true)), '+/', '-_'), '=')
        assert_eq!(
            challenge_for("orcaa"),
            "T9mguEUTOwxZ4vlkIdcJH1-9eTZUTZeLVuotQaMwtmc"
        );
    }

    #[test]
    fn a_matching_callback_resolves_to_the_tenant_handoff_url() {
        let (pending, state) = begin();
        let incoming = Url::parse(&format!(
            "orcaa://auth?state={state}&token=abc&subdomain=clinic"
        ))
        .unwrap();

        let resolved = pending.resolve(&incoming, "orcaa.cloud", "orcaa").unwrap();

        assert_eq!(resolved.url.host_str(), Some("clinic.orcaa.cloud"));
        assert_eq!(resolved.url.path(), "/desktop-handoff");
        assert!(resolved.url.query().unwrap().contains("verifier="));
    }

    #[test]
    fn a_callback_with_the_wrong_state_is_dropped() {
        let (pending, _) = begin();
        let incoming = Url::parse("orcaa://auth?state=forged&token=abc&subdomain=clinic").unwrap();

        assert!(pending.resolve(&incoming, "orcaa.cloud", "orcaa").is_none());
    }

    #[test]
    fn an_unsolicited_callback_is_dropped() {
        let pending = PendingSignIn::default();
        let incoming = Url::parse("orcaa://auth?state=x&token=abc&subdomain=clinic").unwrap();

        assert!(pending.resolve(&incoming, "orcaa.cloud", "orcaa").is_none());
    }

    #[test]
    fn a_callback_cannot_be_replayed() {
        let (pending, state) = begin();
        let incoming = Url::parse(&format!(
            "orcaa://auth?state={state}&token=abc&subdomain=clinic"
        ))
        .unwrap();

        assert!(pending.resolve(&incoming, "orcaa.cloud", "orcaa").is_some());
        assert!(pending.resolve(&incoming, "orcaa.cloud", "orcaa").is_none());
    }

    #[test]
    fn a_crafted_subdomain_cannot_steer_the_webview_off_domain() {
        for hostile in ["evil.com#", "a.b", "../../x", "-lead", "UPPER", ""] {
            let (pending, state) = begin();
            let incoming = Url::parse(&format!(
                "orcaa://auth?state={state}&token=abc&subdomain={hostile}"
            ))
            .unwrap();

            assert!(
                pending.resolve(&incoming, "orcaa.cloud", "orcaa").is_none(),
                "subdomain {hostile:?} should have been rejected"
            );
        }
    }
}
