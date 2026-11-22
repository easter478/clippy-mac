//! Tokens, API keys and passwords live only in the macOS Keychain (service "Clippy"), never in
//! settings.json or the custom actions file. The windows can set or clear one, and ask whether one is
//! set, but never read a value back: only Rust uses them, when it makes the request.

const SERVICE: &str = "Clippy";

/// Command-line test mode only: secrets may come from CLIPPY_SECRET_<NAME> environment variables,
/// so integrations can be tested against mock servers without touching the Keychain.
pub static TEST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn get(name: &str) -> Option<String> {
    if TEST_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        let var = format!("CLIPPY_SECRET_{}", name.to_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
        return std::env::var(var).ok().filter(|s| !s.is_empty());
    }
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::get_generic_password(SERVICE, name).ok().and_then(|b| String::from_utf8(b).ok()).filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        None
    }
}

pub fn set(name: &str, value: &str) -> Result<(), String> {
    if TEST_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(()); // test mode never writes to the Keychain
    }
    #[cfg(target_os = "macos")]
    {
        if value.is_empty() {
            let _ = security_framework::passwords::delete_generic_password(SERVICE, name);
            return Ok(());
        }
        security_framework::passwords::set_generic_password(SERVICE, name, value.as_bytes()).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (name, value);
        Err("Only macOS is supported".into())
    }
}

/// Is a secret saved? Looks at the item's attributes only, never its value, so macOS never shows
/// a Keychain prompt for this check.
pub fn has(name: &str) -> bool {
    if TEST_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        return get(name).is_some();
    }
    #[cfg(target_os = "macos")]
    {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(name)
            .load_attributes(true)
            .load_data(false)
            .limit(Limit::Max(1))
            .search()
            .map(|r| !r.is_empty())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        false
    }
}

/// Secret names are short ids like "home_assistant" or "custom:my-token".
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || "_-:.".contains(c))
}
