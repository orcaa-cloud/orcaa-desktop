//! A stable identifier for THIS COMPUTER.
//!
//! Orcaa can ban a person from the platform, and a ban is only as good as the
//! identifiers it is keyed on. A browser gives up very little: a storage-backed
//! device id (gone when site data is cleared) and a network address (shared
//! and rotating). It cannot read a MAC address — no website can. The desktop
//! shell can do better, because it is a native process: it can ask the
//! operating system which machine it is running on. That answer survives
//! clearing storage, reinstalling the app and switching user accounts in the
//! app.
//!
//! ## What leaves the machine
//!
//! Only a salted SHA-256 of the OS machine id — never the id itself. The id is
//! not secret in any strong sense, but it is the machine's, not ours: other
//! software keys licences off it, and there is no reason for our servers to be
//! able to correlate with that. The salt makes the hash ours alone.
//!
//! ## Where it goes
//!
//! - Published to the hosted app on `window.__ORCAA_SHELL__.machine`
//!   (`shell_page.rs`), which sends it as `X-Hardware-Id`.
//! - Appended to the browser sign-in URL as `dh` (`signin.rs`), so a signup
//!   that starts from the desktop app carries it too.
//!
//! ## Failure is silent and total
//!
//! The release profile is `panic = "abort"`, so nothing here may panic: every
//! path returns `None` on any surprise, and the app simply runs without a
//! hardware id — exactly as every browser does. It is a signal the platform
//! is happy to have, never one the app depends on.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// Versioned so the derivation can change without old and new hashes ever
/// being mistaken for the same scheme.
const SALT: &str = "orcaa.desktop.machine.v1";

static MACHINE: OnceLock<Option<String>> = OnceLock::new();

/// The salted hash of this computer's OS machine id — 64 lowercase hex
/// characters — or `None` when the OS would not say.
///
/// Read once and cached for the life of the process: the answer cannot change
/// while the app is running, and on macOS the read is a subprocess.
pub fn machine_hash() -> Option<&'static str> {
    MACHINE
        .get_or_init(|| raw_machine_id().as_deref().and_then(hash_machine_id))
        .as_deref()
}

/// `None` for an id too short to be a real one (an empty registry value, a
/// blank `/etc/machine-id` in a freshly-built container image): hashing those
/// would give thousands of unrelated machines the same "hardware id".
fn hash_machine_id(raw: &str) -> Option<String> {
    // Case and surrounding braces/whitespace vary by API and OS build for
    // what is the same GUID.
    let id: String = raw
        .trim()
        .trim_matches(|c| c == '{' || c == '}')
        .to_ascii_lowercase();

    if id.len() < 8 {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(SALT.as_bytes());
    hasher.update(b":");
    hasher.update(id.as_bytes());

    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` — written once by
/// Windows Setup and stable until the OS is reinstalled.
///
/// Opened with `KEY_WOW64_64KEY` on purpose: the Windows 7 lane also ships a
/// 32-bit build, and a 32-bit process is otherwise redirected to
/// `WOW6432Node`, where this value does not exist. The classic
/// open/query/close trio (rather than `RegGetValueW` with its newer
/// `RRF_SUBKEY_WOW6464KEY` flag) is what Windows 7 supports.
#[cfg(windows)]
fn raw_machine_id() -> Option<String> {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
        KEY_WOW64_64KEY,
    };

    // A GUID is 36 characters; this is room to spare, and a longer value is
    // simply treated as unreadable.
    const CAPACITY: usize = 128;

    let mut key = HKEY::default();

    // SAFETY: every pointer handed to the registry API below is to a live
    // local, sized exactly as the API is told, and the key is closed on every
    // path after a successful open.
    unsafe {
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Cryptography"),
            None,
            KEY_QUERY_VALUE | KEY_WOW64_64KEY,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return None;
        }

        let mut buffer = [0u16; CAPACITY];
        let mut bytes = (CAPACITY * std::mem::size_of::<u16>()) as u32;

        let status = RegQueryValueExW(
            key,
            w!("MachineGuid"),
            None,
            None,
            Some(buffer.as_mut_ptr().cast::<u8>()),
            Some(&mut bytes),
        );

        let _ = RegCloseKey(key);

        if status != ERROR_SUCCESS {
            return None;
        }

        let units = (bytes as usize / std::mem::size_of::<u16>()).min(CAPACITY);
        let value = String::from_utf16_lossy(&buffer[..units]);

        // REG_SZ data usually, but not always, includes its terminator.
        Some(value.trim_end_matches('\0').to_string())
    }
}

/// `IOPlatformUUID` — the hardware UUID shown in System Information.
#[cfg(target_os = "macos")]
fn raw_machine_id() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // …  "IOPlatformUUID" = "8C5A4F2E-…"
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.contains("IOPlatformUUID"))
        .and_then(|line| line.split('"').nth(3))
        .map(str::to_string)
}

/// systemd's machine id, with the older D-Bus location as a fallback.
#[cfg(all(unix, not(target_os = "macos")))]
fn raw_machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "8c5a4f2e-1b7d-4e3a-9f60-2d1c0b9a8e77";

    #[test]
    fn the_hash_is_64_lowercase_hex_characters() {
        let hash = hash_machine_id(GUID).expect("a real GUID hashes");

        assert_eq!(hash.len(), 64);
        assert!(hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    #[test]
    fn the_same_machine_always_hashes_the_same() {
        assert_eq!(hash_machine_id(GUID), hash_machine_id(GUID));
    }

    #[test]
    fn spelling_differences_in_one_guid_do_not_change_the_hash() {
        // Windows APIs hand the same GUID back upper-cased, braced, or with a
        // trailing newline depending on where it was read from.
        let canonical = hash_machine_id(GUID);

        assert_eq!(hash_machine_id(&GUID.to_uppercase()), canonical);
        assert_eq!(hash_machine_id(&format!("{{{GUID}}}")), canonical);
        assert_eq!(hash_machine_id(&format!("  {GUID}\n")), canonical);
    }

    #[test]
    fn different_machines_hash_differently() {
        assert_ne!(
            hash_machine_id(GUID),
            hash_machine_id("8c5a4f2e-1b7d-4e3a-9f60-2d1c0b9a8e78")
        );
    }

    #[test]
    fn the_raw_id_never_appears_in_the_hash() {
        // The whole point of hashing: the machine's own id stays on the machine.
        let hash = hash_machine_id(GUID).unwrap();

        assert!(!hash.contains("8c5a4f2e"));
        assert_ne!(hash, GUID);
    }

    #[test]
    fn an_empty_or_blank_id_yields_no_hash_rather_than_a_shared_one() {
        // A blank /etc/machine-id (fresh container image) must not give every
        // such machine the same "hardware id".
        assert_eq!(hash_machine_id(""), None);
        assert_eq!(hash_machine_id("   \n"), None);
        assert_eq!(hash_machine_id("{}"), None);
    }

    #[test]
    fn reading_this_machine_never_panics_and_is_stable() {
        // Whatever the OS answers — an id or nothing — asking twice agrees,
        // and a present answer has the shape the backend accepts.
        let first = machine_hash();

        assert_eq!(first, machine_hash());

        if let Some(hash) = first {
            assert_eq!(hash.len(), 64);
        }
    }
}
