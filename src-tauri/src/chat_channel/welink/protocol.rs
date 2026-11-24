use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::chat_channel::error::ChatChannelError;

pub const DEFAULT_WS_PATH: &str = "/appstore-gateway/ws/agent";
pub const DEFAULT_TOOL_TYPE: &str = "codeagent";
pub const CLOUD_PROTOCOL: &str = "cloud";
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const TOOL_DONE_COMPAT_DELAY: Duration = Duration::from_millis(100);
pub const SUPPRESS_REPLY_TEXT: &str = "本机器人不处理群聊消息，请勿在群内@提问";

const BASE62_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Normalize a user-supplied WeLink gateway address into a WebSocket endpoint URL.
///
/// Users only need to provide a domain/host (e.g. `api.inner.welink.example.com`),
/// which expands to `ws://<host>/appstore-gateway/ws/agent`. Full `ws://`, `wss://`,
/// `http://`, or `https://` URLs are also accepted and normalized.
pub fn normalize_gateway_ws_url(input: &str) -> Result<String, ChatChannelError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ChatChannelError::ConfigurationInvalid(
            "gateway_url is required".into(),
        ));
    }

    let candidate = if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed.to_string()
    } else if let Some(rest) = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
    {
        format!("ws://{rest}")
    } else {
        format!("ws://{trimmed}")
    };

    let mut parsed = url::Url::parse(&candidate).map_err(|e| {
        ChatChannelError::ConfigurationInvalid(format!("Invalid WeLink gateway URL '{input}': {e}"))
    })?;

    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err(ChatChannelError::ConfigurationInvalid(format!(
            "Invalid WeLink gateway URL '{input}': missing host"
        )));
    }

    if parsed.path().is_empty() || parsed.path() == "/" {
        parsed.set_path(DEFAULT_WS_PATH);
    }

    Ok(parsed.to_string())
}

/// Derive the HTTPS origin for the WeLink registration API from a gateway domain or URL.
///
/// Always uses `https://{host}[:port]` with no trailing slash or path.
pub fn derive_register_base_url(input: &str) -> Result<String, ChatChannelError> {
    let ws_url = normalize_gateway_ws_url(input)?;
    let parsed = url::Url::parse(&ws_url).map_err(|e| {
        ChatChannelError::ConfigurationInvalid(format!("Invalid WeLink gateway URL '{input}': {e}"))
    })?;

    let host = parsed.host_str().ok_or_else(|| {
        ChatChannelError::ConfigurationInvalid(format!(
            "Invalid WeLink gateway URL '{input}': missing host"
        ))
    })?;

    match parsed.port() {
        Some(port) => Ok(format!("https://{host}:{port}")),
        None => Ok(format!("https://{host}")),
    }
}

/// Build the `Sec-WebSocket-Protocol` authentication header value (`auth.<base64url>`).
pub fn build_auth_subprotocol(ak: &str, sk: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    build_auth_subprotocol_with(ak, sk, &ts, &nonce)
}

/// Deterministic variant of [`build_auth_subprotocol`] for unit tests.
pub fn build_auth_subprotocol_with(ak: &str, sk: &str, ts: &str, nonce: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(sk.as_bytes())
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(format!("{ak}{ts}{nonce}").as_bytes());
    let sign = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let payload = serde_json::json!({
        "ak": ak,
        "ts": ts,
        "nonce": nonce,
        "sign": sign,
    });
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&payload).expect("auth payload serializes"));
    format!("auth.{encoded}")
}

/// Generate a 12-character random alphanumeric `toolSessionId`.
pub fn generate_tool_session_id() -> String {
    random_base62(12)
}

/// Generate a unique short `messageId` for a single assistant reply turn.
pub fn generate_message_id() -> String {
    format!("msg_{}", random_base62(12))
}

fn random_base62(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..BASE62_ALPHABET.len());
            BASE62_ALPHABET[idx] as char
        })
        .collect()
}

/// Best-effort local hostname for `register.deviceName`.
pub fn get_device_hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: `gethostname` writes at most `buf.len()` bytes into `buf`.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
        if rc == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if let Ok(name) = std::str::from_utf8(&buf[..len]) {
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }

    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    "codeg".to_string()
}

/// Classification of WebSocket close codes from the WeLink gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Auth/conflict terminal codes (`4403`, `4409`): mark channel `Error` and stop reconnecting.
    Terminal,
    /// Known transient codes (`1006`, `1012`, `1013`, `4408`): reconnect with backoff.
    Reconnect,
    /// Any other close code: log warning and reconnect (always-on semantics).
    ReconnectWithWarn,
}

pub fn classify_close_code(code: u16) -> CloseAction {
    match code {
        4403 | 4409 => CloseAction::Terminal,
        1006 | 1012 | 1013 | 4408 => CloseAction::Reconnect,
        _ => CloseAction::ReconnectWithWarn,
    }
}

// ── Upstream messages (Client -> Gateway) ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub device_name: String,
    pub os: String,
    pub tool_type: String,
    pub tool_version: String,
    pub plugin_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreatedMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub welink_session_id: String,
    pub tool_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub session: SessionSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub properties: serde_json::Value,
    pub protocol: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEventMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub tool_session_id: String,
    pub event: CloudEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDoneMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub tool_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub welink_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolErrorMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub welink_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_session_id: Option<String>,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponseMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub opencode_online: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommandEntry {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommandsPayload {
    pub slash_commands: Vec<SlashCommandEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommandsResultMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub tool_session_id: String,
    pub trace_id: String,
    pub payload: SlashCommandsPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum UpstreamMessage {
    Register(RegisterMessage),
    Heartbeat(HeartbeatMessage),
    SessionCreated(SessionCreatedMessage),
    ToolEvent(ToolEventMessage),
    ToolDone(ToolDoneMessage),
    ToolError(ToolErrorMessage),
    StatusResponse(StatusResponseMessage),
    SlashCommandsResult(SlashCommandsResultMessage),
}

impl UpstreamMessage {
    pub fn register(
        device_name: String,
        tool_type: String,
        mac_address: Option<String>,
    ) -> Self {
        let version = env!("CARGO_PKG_VERSION").to_string();
        Self::Register(RegisterMessage {
            msg_type: "register",
            device_name,
            os: std::env::consts::OS.to_string(),
            tool_type: if tool_type.trim().is_empty() {
                DEFAULT_TOOL_TYPE.to_string()
            } else {
                tool_type
            },
            tool_version: version.clone(),
            plugin_version: version,
            mac_address: mac_address.filter(|m| !m.trim().is_empty()),
        })
    }

    pub fn heartbeat() -> Self {
        Self::Heartbeat(HeartbeatMessage {
            msg_type: "heartbeat",
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }

    pub fn session_created(
        welink_session_id: String,
        tool_session_id: String,
        trace_id: Option<String>,
        title: Option<String>,
    ) -> Self {
        Self::SessionCreated(SessionCreatedMessage {
            msg_type: "session_created",
            welink_session_id,
            tool_session_id: tool_session_id.clone(),
            trace_id,
            session: SessionSummary {
                id: tool_session_id,
                title,
            },
        })
    }

    pub fn step_start(tool_session_id: String, message_id: &str) -> Self {
        Self::ToolEvent(ToolEventMessage {
            msg_type: "tool_event",
            tool_session_id,
            event: CloudEvent {
                event_type: "step.start",
                properties: serde_json::json!({ "messageId": message_id }),
                protocol: CLOUD_PROTOCOL,
            },
        })
    }

    pub fn text_done(
        tool_session_id: String,
        message_id: &str,
        part_id: &str,
        content: &str,
    ) -> Self {
        Self::ToolEvent(ToolEventMessage {
            msg_type: "tool_event",
            tool_session_id,
            event: CloudEvent {
                event_type: "text.done",
                properties: serde_json::json!({
                    "messageId": message_id,
                    "partId": part_id,
                    "content": content,
                }),
                protocol: CLOUD_PROTOCOL,
            },
        })
    }

    pub fn step_done(tool_session_id: String, message_id: &str) -> Self {
        Self::ToolEvent(ToolEventMessage {
            msg_type: "tool_event",
            tool_session_id,
            event: CloudEvent {
                event_type: "step.done",
                properties: serde_json::json!({ "messageId": message_id }),
                protocol: CLOUD_PROTOCOL,
            },
        })
    }

    pub fn tool_done(tool_session_id: String, welink_session_id: Option<String>) -> Self {
        Self::ToolDone(ToolDoneMessage {
            msg_type: "tool_done",
            tool_session_id,
            welink_session_id,
        })
    }

    pub fn tool_error(
        welink_session_id: Option<String>,
        tool_session_id: Option<String>,
        error: impl Into<String>,
        reason: Option<String>,
    ) -> Self {
        Self::ToolError(ToolErrorMessage {
            msg_type: "tool_error",
            welink_session_id,
            tool_session_id,
            error: error.into(),
            reason,
        })
    }

    pub fn status_response(online: bool) -> Self {
        Self::StatusResponse(StatusResponseMessage {
            msg_type: "status_response",
            opencode_online: online,
        })
    }

    pub fn slash_commands_result(
        tool_session_id: String,
        trace_id: String,
        commands: Vec<SlashCommandEntry>,
    ) -> Self {
        Self::SlashCommandsResult(SlashCommandsResultMessage {
            msg_type: "slash_commands_result",
            tool_session_id,
            trace_id,
            payload: SlashCommandsPayload {
                slash_commands: commands,
            },
        })
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// ── Downstream messages (Gateway -> Client) ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeEnvelope {
    pub action: String,
    #[serde(default)]
    pub ak: Option<String>,
    #[serde(default)]
    pub welink_session_id: Option<String>,
    #[serde(default)]
    pub tool_session_id: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub assistant_account: Option<String>,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    #[serde(default)]
    pub suppress_reply: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionPayload {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub assistant_id: Option<String>,
    #[serde(default)]
    pub ext_parameters: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl CreateSessionPayload {
    /// Extract user employee ID from `extParameters.platformExtParam.businessSessionId`.
    pub fn business_session_id(&self) -> Option<&str> {
        self.ext_parameters
            .as_ref()?
            .pointer("/platformExtParam/businessSessionId")?
            .as_str()
            .filter(|s| !s.trim().is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPayload {
    #[serde(default)]
    pub text: String,
    pub tool_session_id: String,
    #[serde(default)]
    pub assistant_account: Option<String>,
    #[serde(default)]
    pub send_user_account: Option<String>,
    #[serde(default)]
    pub im_group_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub ext_parameters: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdPayload {
    pub tool_session_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum DownstreamMessage {
    RegisterOk,
    RegisterRejected { reason: String },
    StatusQuery,
    Invoke(Box<InvokeEnvelope>),
    Unknown(serde_json::Value),
}

pub fn parse_downstream(raw: &str) -> Result<DownstreamMessage, serde_json::Error> {
    let val: serde_json::Value = serde_json::from_str(raw)?;
    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match msg_type {
        "register_ok" => Ok(DownstreamMessage::RegisterOk),
        "register_rejected" => {
            let reason = val
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("registration rejected")
                .to_string();
            Ok(DownstreamMessage::RegisterRejected { reason })
        }
        "status_query" => Ok(DownstreamMessage::StatusQuery),
        "invoke" => {
            let env: InvokeEnvelope = serde_json::from_value(val)?;
            Ok(DownstreamMessage::Invoke(Box::new(env)))
        }
        _ => Ok(DownstreamMessage::Unknown(val)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_gateway_url_accepts_domain_and_full_urls() {
        assert_eq!(
            normalize_gateway_ws_url("api.inner.welink.example.com").unwrap(),
            "ws://api.inner.welink.example.com/appstore-gateway/ws/agent"
        );
        assert_eq!(
            normalize_gateway_ws_url("  api.inner.welink.example.com:8443/ ").unwrap(),
            "ws://api.inner.welink.example.com:8443/appstore-gateway/ws/agent"
        );
        assert_eq!(
            normalize_gateway_ws_url("ws://api.inner.welink.example.com/appstore-gateway/ws/agent")
                .unwrap(),
            "ws://api.inner.welink.example.com/appstore-gateway/ws/agent"
        );
        assert_eq!(
            normalize_gateway_ws_url("wss://uat.welink.example.com").unwrap(),
            "wss://uat.welink.example.com/appstore-gateway/ws/agent"
        );
        assert_eq!(
            normalize_gateway_ws_url("https://uat.welink.example.com").unwrap(),
            "ws://uat.welink.example.com/appstore-gateway/ws/agent"
        );
        assert!(normalize_gateway_ws_url("").is_err());
        assert!(normalize_gateway_ws_url("   ").is_err());
    }

    #[test]
    fn derive_register_base_url_uses_https_origin() {
        assert_eq!(
            derive_register_base_url("api.inner.welink.example.com").unwrap(),
            "https://api.inner.welink.example.com"
        );
        assert_eq!(
            derive_register_base_url("ws://api.inner.welink.example.com:9443/appstore-gateway/ws/agent")
                .unwrap(),
            "https://api.inner.welink.example.com:9443"
        );
        assert_eq!(
            derive_register_base_url("wss://uat.welink.example.com/custom/path").unwrap(),
            "https://uat.welink.example.com"
        );
    }

    #[test]
    fn build_auth_subprotocol_matches_hmac_sha256_contract() {
        let subprotocol =
            build_auth_subprotocol_with("ak_test", "sk_secret", "1700000000", "nonce-1234");
        assert!(subprotocol.starts_with("auth."));
        let b64 = &subprotocol["auth.".len()..];
        assert!(!b64.contains('='));
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(b64)
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(json["ak"], "ak_test");
        assert_eq!(json["ts"], "1700000000");
        assert_eq!(json["nonce"], "nonce-1234");

        // Recompute expected HMAC-SHA256 independently
        let mut mac = Hmac::<Sha256>::new_from_slice(b"sk_secret").unwrap();
        mac.update(b"ak_test1700000000nonce-1234");
        let expected =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(json["sign"], expected);
    }

    #[test]
    fn parses_real_captured_create_session_and_chat_frames() {
        let create_raw = r#"{"type":"invoke","ak":"20260714110920195180543","welinkSessionId":"202670114230673408","traceId":"5r_r0x9lE4hRXR7qd6qE1lzpz9ERP4re","action":"create_session","payload":{"title":"","extParameters":{"businessExtParam":{},"platformExtParam":{"businessSessionDomain":"miniapp","businessSessionType":"direct","businessSessionId":"j00466872","bizRobotTag":null}}}}"#;
        let msg = parse_downstream(create_raw).unwrap();
        let DownstreamMessage::Invoke(env) = msg else {
            panic!("expected Invoke");
        };
        assert_eq!(env.action, "create_session");
        assert_eq!(
            env.welink_session_id.as_deref(),
            Some("202670114230673408")
        );
        assert_eq!(
            env.trace_id.as_deref(),
            Some("5r_r0x9lE4hRXR7qd6qE1lzpz9ERP4re")
        );
        let payload: CreateSessionPayload =
            serde_json::from_value(env.payload.unwrap()).unwrap();
        assert_eq!(payload.business_session_id(), Some("j00466872"));

        // Minimal create_session without extParameters must also succeed
        let minimal: CreateSessionPayload =
            serde_json::from_value(serde_json::json!({ "title": "hi" })).unwrap();
        assert_eq!(minimal.title.as_deref(), Some("hi"));
        assert_eq!(minimal.business_session_id(), None);

        let chat_raw = r#"{"type":"invoke","ak":"20260714110920195180543","traceId":"5r_r0x9lE4hRXR7qd6qE1lzpz9ERP4re","action":"chat","assistantAccount":"dig_00466872_mrk2oj7x","payload":{"text":"hello world","toolSessionId":"SqhqGmnpeGCR","assistantAccount":"dig_00466872_mrk2oj7x","sendUserAccount":"j00466872","imGroupId":"","messageId":"1784010736526","extParameters":{}}}"#;
        let msg = parse_downstream(chat_raw).unwrap();
        let DownstreamMessage::Invoke(env) = msg else {
            panic!("expected Invoke");
        };
        assert_eq!(env.action, "chat");
        assert!(env.welink_session_id.is_none());
        let chat: ChatPayload = serde_json::from_value(env.payload.unwrap()).unwrap();
        assert_eq!(chat.text, "hello world");
        assert_eq!(chat.tool_session_id, "SqhqGmnpeGCR");
        assert_eq!(chat.send_user_account.as_deref(), Some("j00466872"));
        assert_eq!(chat.message_id.as_deref(), Some("1784010736526"));
    }

    #[test]
    fn serializes_cloud_tool_events_and_protocol_free_tool_done() {
        let start: serde_json::Value = serde_json::from_str(
            &UpstreamMessage::step_start("SqhqGmnpeGCR".into(), "m1")
                .to_json_string()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(start["type"], "tool_event");
        assert_eq!(start["toolSessionId"], "SqhqGmnpeGCR");
        assert_eq!(start["event"]["type"], "step.start");
        assert_eq!(start["event"]["protocol"], "cloud");
        assert_eq!(start["event"]["properties"]["messageId"], "m1");

        let done_event: serde_json::Value = serde_json::from_str(
            &UpstreamMessage::text_done("SqhqGmnpeGCR".into(), "m1", "m1-p0", "hello")
                .to_json_string()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(done_event["event"]["type"], "text.done");
        assert_eq!(done_event["event"]["protocol"], "cloud");
        assert_eq!(done_event["event"]["properties"]["partId"], "m1-p0");
        assert_eq!(done_event["event"]["properties"]["content"], "hello");

        let tool_done: serde_json::Value = serde_json::from_str(
            &UpstreamMessage::tool_done("SqhqGmnpeGCR".into(), Some("202670114230673408".into()))
                .to_json_string()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(tool_done["type"], "tool_done");
        assert_eq!(tool_done["toolSessionId"], "SqhqGmnpeGCR");
        assert_eq!(tool_done["welinkSessionId"], "202670114230673408");
        assert!(tool_done.get("protocol").is_none());
    }

    #[test]
    fn classifies_ws_close_codes() {
        assert_eq!(classify_close_code(4403), CloseAction::Terminal);
        assert_eq!(classify_close_code(4409), CloseAction::Terminal);
        assert_eq!(classify_close_code(1006), CloseAction::Reconnect);
        assert_eq!(classify_close_code(1012), CloseAction::Reconnect);
        assert_eq!(classify_close_code(1013), CloseAction::Reconnect);
        assert_eq!(classify_close_code(4408), CloseAction::Reconnect);
        assert_eq!(classify_close_code(1000), CloseAction::ReconnectWithWarn);
    }
}
