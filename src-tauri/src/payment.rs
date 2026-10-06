//! A payment return can only focus a checkout registered by the current Orcaa
//! window. It carries no authentication and cannot settle a payment.
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, WebviewWindow};
use url::Url;

#[derive(Default)]
pub struct PendingPayment(Mutex<Option<Attempt>>);

struct Attempt {
    payment: String,
    nonce: String,
    origin: String,
    expires_at: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn valid_payment(value: &str) -> bool {
    value.len() == 36 && value.bytes().enumerate().all(|(i, ch)| {
        if [8, 13, 18, 23].contains(&i) { ch == b'-' } else { ch.is_ascii_hexdigit() }
    })
}

impl Attempt {
    fn matches(&self, url: &Url, origin: &str, now: u64) -> bool {
        let pairs: Vec<_> = url.query_pairs().collect();
        url.scheme() == "orcaa" && url.host_str() == Some("payment-return")
            && url.username().is_empty() && url.password().is_none() && url.port().is_none()
            && (url.path().is_empty() || url.path() == "/") && url.fragment().is_none()
            && pairs.len() == 2 && self.expires_at >= now && self.origin == origin
            && pairs.iter().filter(|(k, v)| k == "payment" && v == &self.payment).count() == 1
            && pairs.iter().filter(|(k, v)| k == "nonce" && v == &self.nonce).count() == 1
    }
}

#[tauri::command]
pub fn shell_payment_register(window: WebviewWindow, state: tauri::State<'_, PendingPayment>, payment_id: String, nonce: String, expires_at: u64) -> Result<(), String> {
    let current = window.url().map_err(|_| "Unavailable window")?;
    let now = now_ms();
    if window.label() != "main" || !crate::is_orcaa_host(&current)
        || !matches!(current.scheme(), "https" | "http") || !valid_payment(&payment_id)
        || nonce.len() != 64 || !nonce.bytes().all(|ch| ch.is_ascii_alphanumeric())
        || expires_at <= now || expires_at > now + 3_660_000 {
        return Err("Invalid payment registration".into());
    }
    *state.0.lock().map_err(|_| "Unavailable payment state")? = Some(Attempt {
        payment: payment_id, nonce, origin: current.origin().ascii_serialization(), expires_at,
    });
    Ok(())
}

pub fn complete(app: &AppHandle, url: &Url) {
    let Some(window) = app.get_webview_window("main") else { return; };
    let Ok(current) = window.url() else { return; };
    let state = app.state::<PendingPayment>();
    let Ok(mut pending) = state.0.lock() else { return; };
    if pending.as_ref().is_some_and(|attempt| attempt.matches(url, &current.origin().ascii_serialization(), now_ms())) {
        *pending = None;
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.eval("window.dispatchEvent(new Event('orcaa-payment-return'))");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_registered_origin_nonce_and_attempt_can_return() {
        let attempt = Attempt { payment: "12345678-1234-1234-1234-123456789abc".into(), nonce: "a".repeat(64), origin: "https://shop.orcaa.cloud".into(), expires_at: 1000 };
        let url = Url::parse(&format!("orcaa://payment-return?payment={}&nonce={}", attempt.payment, attempt.nonce)).unwrap();
        assert!(valid_payment(&attempt.payment));
        assert!(attempt.matches(&url, &attempt.origin, 999));
        assert!(!attempt.matches(&url, "https://other.orcaa.cloud", 999));
        assert!(!attempt.matches(&url, &attempt.origin, 1001));
        assert!(!attempt.matches(&Url::parse(&format!("{}&nonce=wrong", url)).unwrap(), &attempt.origin, 999));
        assert!(!attempt.matches(&Url::parse(&url.as_str().replace("payment-return", "auth")).unwrap(), &attempt.origin, 999));
    }
}
