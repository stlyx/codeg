use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Lark,
    Telegram,
    Welink,
    Weixin,
}

// ── Per-channel strong typed configs ──

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub chat_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LarkConfig {
    pub app_id: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeixinConfig {
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WelinkConfig {
    pub group_id: String,
    pub send_http_url: String,
    #[serde(default = "default_welink_cli_path")]
    pub welink_cli_path: String,
    #[serde(default)]
    pub include_sender: Vec<String>,
    #[serde(default)]
    pub exclude_sender: Vec<String>,
    #[serde(default = "default_welink_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

fn default_welink_cli_path() -> String {
    "welink-cli".to_string()
}

fn default_welink_request_timeout_ms() -> u64 {
    10_000
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::Lark => write!(f, "lark"),
            ChannelType::Telegram => write!(f, "telegram"),
            ChannelType::Welink => write!(f, "welink"),
            ChannelType::Weixin => write!(f, "weixin"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone)]
pub struct SentMessageId(pub String);

pub struct IncomingCommand {
    pub channel_id: i32,
    pub sender_id: String,
    pub command_text: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct RichMessage {
    pub title: Option<String>,
    pub body: String,
    pub fields: Vec<(String, String)>,
    pub level: MessageLevel,
}

impl RichMessage {
    pub fn info(body: impl Into<String>) -> Self {
        Self {
            title: None,
            body: body.into(),
            fields: Vec::new(),
            level: MessageLevel::Info,
        }
    }

    pub fn error(body: impl Into<String>) -> Self {
        Self {
            title: None,
            body: body.into(),
            fields: Vec::new(),
            level: MessageLevel::Error,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    pub fn to_plain_text(&self) -> String {
        let mut text = String::new();
        if let Some(title) = &self.title {
            text.push_str(title);
            text.push('\n');
        }
        text.push_str(&self.body);
        for (key, value) in &self.fields {
            text.push_str(&format!("\n{}: {}", key, value));
        }
        text
    }
}

// ── Phase 2 forward-compatible types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Danger,
    Default,
}

#[derive(Debug, Clone)]
pub struct MessageButton {
    pub id: String,
    pub label: String,
    pub style: ButtonStyle,
}

#[derive(Debug, Clone)]
pub struct InteractiveMessage {
    pub base: RichMessage,
    pub buttons: Vec<MessageButton>,
    pub callback_context: serde_json::Value,
}

impl InteractiveMessage {
    pub fn to_rich_fallback(&self) -> RichMessage {
        let mut msg = self.base.clone();
        if !self.buttons.is_empty() {
            let button_text: Vec<String> = self
                .buttons
                .iter()
                .map(|b| format!("[{}]", b.label))
                .collect();
            msg.body
                .push_str(&format!("\n\n{}", button_text.join("  ")));
        }
        msg
    }
}
