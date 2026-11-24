use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::protocol::derive_register_base_url;
use crate::chat_channel::error::ChatChannelError;

/// Shared HTTP client dedicated to WeLink internal registration endpoints.
///
/// The WeLink corporate gateway serves internal self-signed certificates on the
/// registration host, so `danger_accept_invalid_certs(true)` is scoped strictly
/// to this dedicated client and never affects any other HTTP client in codeg.
fn register_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default()
        })
        .clone()
}

// ── Public types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WelinkRegisterInfo {
    pub pc_url: String,
    pub qrcode: String,
    pub access_token: String,
    pub expire_time: String,
}

/// Internal status carrying AK/SK credentials consumed by the `_core` command layer.
#[derive(Debug, Clone)]
pub struct WelinkRegisterStatus {
    pub status: String,
    pub ak: Option<String>,
    pub sk: Option<String>,
    pub assistant_name: Option<String>,
}

/// Frontend-safe subset of [`WelinkRegisterStatus`] — never exposes AK/SK.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WelinkRegisterStatusPublic {
    pub status: String,
}

// ── Registration HTTP API ──

pub async fn apply_register_url(
    gateway_url: &str,
    channel: &str,
) -> Result<WelinkRegisterInfo, ChatChannelError> {
    let base_url = derive_register_base_url(gateway_url)?;
    let mac = get_first_mac().unwrap_or_default();
    let url = format!("{base_url}/assistant-api/nologin/we-crew/im-register/qrcode");

    let resp = register_client()
        .post(&url)
        .json(&serde_json::json!({
            "channel": channel,
            "mac": mac,
        }))
        .send()
        .await
        .map_err(|e| {
            ChatChannelError::ConnectionFailed(format!("WeLink register URL request failed: {e}"))
        })?;

    let http_status = resp.status();
    if !http_status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink register URL HTTP {http_status}: {body}"
        )));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        ChatChannelError::ConnectionFailed(format!("WeLink register URL parse failed: {e}"))
    })?;

    parse_apply_register_response(&body)
}

pub async fn check_register_status(
    gateway_url: &str,
    qrcode: &str,
    access_token: &str,
) -> Result<WelinkRegisterStatus, ChatChannelError> {
    let base_url = derive_register_base_url(gateway_url)?;
    let encoded_qr = urlencoding::encode(qrcode);
    let url =
        format!("{base_url}/assistant-api/nologin/we-crew/im-register/qrcode-detail/{encoded_qr}");

    let resp = register_client()
        .get(&url)
        .header("qrcodeToken", access_token)
        .send()
        .await
        .map_err(|e| {
            ChatChannelError::ConnectionFailed(format!(
                "WeLink register status request failed: {e}"
            ))
        })?;

    let http_status = resp.status();
    if !http_status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink register status HTTP {http_status}: {body}"
        )));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        ChatChannelError::ConnectionFailed(format!("WeLink register status parse failed: {e}"))
    })?;

    parse_check_register_response(&body)
}

fn is_code_ok(body: &serde_json::Value) -> bool {
    match body.get("code") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(200),
        Some(serde_json::Value::String(s)) => s.trim() == "200",
        _ => false,
    }
}

fn required_non_empty_string(
    data: &serde_json::Value,
    field: &str,
) -> Result<String, ChatChannelError> {
    let val = match data.get(field) {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    if val.is_empty() {
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink register response missing non-empty field '{field}'"
        )));
    }
    Ok(val)
}

fn parse_apply_register_response(
    body: &serde_json::Value,
) -> Result<WelinkRegisterInfo, ChatChannelError> {
    if !is_code_ok(body) {
        let msg = body
            .get("message")
            .or_else(|| body.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("unexpected response code");
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink register apply failed: {msg}"
        )));
    }

    let data = body.get("data").ok_or_else(|| {
        ChatChannelError::ConnectionFailed("WeLink register response missing data".into())
    })?;

    // Validate all five required fields per contract (`weUrl` verified non-empty even though stage 1 only displays `pcUrl`).
    let qrcode = required_non_empty_string(data, "qrcode")?;
    let access_token = required_non_empty_string(data, "accessToken")?;
    let _we_url = required_non_empty_string(data, "weUrl")?;
    let pc_url = required_non_empty_string(data, "pcUrl")?;
    let expire_time = required_non_empty_string(data, "expireTime")?;

    Ok(WelinkRegisterInfo {
        pc_url,
        qrcode,
        access_token,
        expire_time,
    })
}

fn is_expired_flag(val: Option<&serde_json::Value>) -> bool {
    match val {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
        Some(serde_json::Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
        }
        _ => false,
    }
}

fn parse_check_register_response(
    body: &serde_json::Value,
) -> Result<WelinkRegisterStatus, ChatChannelError> {
    if !is_code_ok(body) {
        let msg = body
            .get("message")
            .or_else(|| body.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("unexpected response code");
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink register status failed: {msg}"
        )));
    }

    let data = body.get("data").ok_or_else(|| {
        ChatChannelError::ConnectionFailed("WeLink register status missing data".into())
    })?;

    // Check `expired` first (independent of `status` per contract).
    if is_expired_flag(data.get("expired")) {
        return Ok(WelinkRegisterStatus {
            status: "expired".to_string(),
            ak: None,
            sk: None,
            assistant_name: None,
        });
    }

    let status_code = match data.get("status") {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };

    let status = match status_code {
        Some(0) => "waiting",
        Some(1) => "scanned",
        Some(2) => "confirmed",
        Some(3) => "cancelled",
        _ => "waiting",
    }
    .to_string();

    let ak = data
        .get("ak")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let sk = data
        .get("sk")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let assistant_name = data
        .get("name")
        .or_else(|| data.get("nameEn"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    Ok(WelinkRegisterStatus {
        status,
        ak,
        sk,
        assistant_name,
    })
}

// ── MAC address discovery ──

/// Normalize and validate a 6-octet MAC address, rejecting all-zero values.
fn normalize_mac(raw: &str) -> Option<String> {
    let normalized = raw.trim().replace('-', ":").to_ascii_lowercase();
    let parts: Vec<&str> = normalized.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut all_zero = true;
    for part in &parts {
        if part.len() != 2 || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        if *part != "00" {
            all_zero = false;
        }
    }
    if all_zero {
        return None;
    }
    Some(normalized)
}

/// Return the first non-loopback, non-zero hardware MAC address on this machine.
/// Returns `None` when unavailable (callers omit `macAddress` or pass `""`).
pub fn get_first_mac() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let mut entries: Vec<_> = std::fs::read_dir("/sys/class/net")
            .ok()?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name();
            if name == "lo" {
                continue;
            }
            let base = entry.path();
            // Skip loopback interfaces (`IFF_LOOPBACK = 0x8`).
            if let Ok(flags_raw) = std::fs::read_to_string(base.join("flags")) {
                let hex = flags_raw.trim().trim_start_matches("0x");
                if let Ok(flags) = u32::from_str_radix(hex, 16) {
                    if flags & 0x8 != 0 {
                        continue;
                    }
                }
            }
            if let Ok(addr) = std::fs::read_to_string(base.join("address")) {
                if let Some(mac) = normalize_mac(&addr) {
                    return Some(mac);
                }
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ifconfig").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut is_loopback = false;
        for line in stdout.lines() {
            if !line.starts_with('\t') && !line.starts_with(' ') && line.contains(':') {
                is_loopback = line.starts_with("lo") || line.contains("LOOPBACK");
            } else if !is_loopback {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("ether ") {
                    if let Some(token) = rest.split_whitespace().next() {
                        if let Some(mac) = normalize_mac(token) {
                            return Some(mac);
                        }
                    }
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("getmac")
            .args(["/fo", "csv", "/nh"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(first_col) = line.split(',').next() {
                let candidate = first_col.trim().trim_matches('"');
                if let Some(mac) = normalize_mac(candidate) {
                    return Some(mac);
                }
            }
        }
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apply_register_response_for_numeric_and_string_code() {
        let numeric = serde_json::json!({
            "code": 200,
            "data": {
                "qrcode": "qr-1",
                "accessToken": "tok-1",
                "weUrl": "welink://qr",
                "pcUrl": "https://example.com/auth",
                "expireTime": "2026-11-24T20:00:00Z"
            }
        });
        let info = parse_apply_register_response(&numeric).unwrap();
        assert_eq!(info.pc_url, "https://example.com/auth");
        assert_eq!(info.qrcode, "qr-1");
        assert_eq!(info.access_token, "tok-1");

        let string_code = serde_json::json!({
            "code": "200",
            "data": {
                "qrcode": "qr-2",
                "accessToken": "tok-2",
                "weUrl": "welink://qr2",
                "pcUrl": "https://example.com/auth2",
                "expireTime": 1784010736526i64
            }
        });
        let info2 = parse_apply_register_response(&string_code).unwrap();
        assert_eq!(info2.expire_time, "1784010736526");

        // Missing weUrl must fail validation
        let incomplete = serde_json::json!({
            "code": "200",
            "data": {
                "qrcode": "qr-2",
                "accessToken": "tok-2",
                "weUrl": "",
                "pcUrl": "https://example.com/auth2",
                "expireTime": "123"
            }
        });
        assert!(parse_apply_register_response(&incomplete).is_err());
    }

    #[test]
    fn parses_check_register_status_with_expired_precedence() {
        for expired_val in [
            serde_json::json!(true),
            serde_json::json!(1),
            serde_json::json!("true"),
            serde_json::json!("1"),
            serde_json::json!("YES"),
        ] {
            let body = serde_json::json!({
                "code": 200,
                "data": { "status": 0, "expired": expired_val }
            });
            assert_eq!(parse_check_register_response(&body).unwrap().status, "expired");
        }

        let confirmed = serde_json::json!({
            "code": "200",
            "data": {
                "status": 2,
                "ak": "ak-value",
                "sk": "sk-value",
                "name": "My Assistant",
                "expired": false
            }
        });
        let parsed = parse_check_register_response(&confirmed).unwrap();
        assert_eq!(parsed.status, "confirmed");
        assert_eq!(parsed.ak.as_deref(), Some("ak-value"));
        assert_eq!(parsed.sk.as_deref(), Some("sk-value"));
        assert_eq!(parsed.assistant_name.as_deref(), Some("My Assistant"));

        let scanned = serde_json::json!({ "code": 200, "data": { "status": "1" } });
        assert_eq!(parse_check_register_response(&scanned).unwrap().status, "scanned");

        let cancelled = serde_json::json!({ "code": 200, "data": { "status": 3 } });
        assert_eq!(parse_check_register_response(&cancelled).unwrap().status, "cancelled");
    }

    #[test]
    fn normalizes_mac_and_rejects_zeros() {
        assert_eq!(
            normalize_mac("AA-BB-CC-DD-EE-FF"),
            Some("aa:bb:cc:dd:ee:ff".to_string())
        );
        assert_eq!(normalize_mac("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac("not-a-mac"), None);
    }
}
