use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

use crate::chat_channel::error::ChatChannelError;
use crate::chat_channel::traits::ChatChannelBackend;
use crate::chat_channel::types::*;

const QUERY_COUNT: &str = "100";
const POLL_INTERVAL_SECS: u64 = 5;
const RETRY_INTERVAL_SECS: u64 = 10;
const MAX_CONTENT_CHARS: usize = 3_000;

pub struct WelinkBackend {
    channel_id: i32,
    token: String,
    group_id: String,
    send_http_url: String,
    welink_cli_path: String,
    include_sender: Vec<String>,
    exclude_sender: Vec<String>,
    request_timeout: Duration,
    client: reqwest::Client,
    status: std::sync::Arc<Mutex<ChannelConnectionStatus>>,
    shutdown_tx: std::sync::Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
}

impl WelinkBackend {
    pub fn new(channel_id: i32, token: String, config: WelinkConfig) -> Self {
        let request_timeout = Duration::from_millis(config.request_timeout_ms.max(1));
        Self {
            channel_id,
            token,
            group_id: config.group_id,
            send_http_url: config.send_http_url,
            welink_cli_path: config.welink_cli_path,
            include_sender: normalize_sender_list(config.include_sender),
            exclude_sender: normalize_sender_list(config.exclude_sender),
            request_timeout,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(request_timeout)
                .build()
                .unwrap_or_default(),
            status: std::sync::Arc::new(Mutex::new(ChannelConnectionStatus::Disconnected)),
            shutdown_tx: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    async fn run_auth_login(&self) -> Result<(), ChatChannelError> {
        run_cli_command(auth_login_args(&self.welink_cli_path), self.request_timeout)
            .await
            .map(|_| ())
            .map_err(|e| ChatChannelError::AuthenticationFailed(e.to_string()))
    }

    async fn query_history(&self, cursor: Option<&str>) -> Result<String, ChatChannelError> {
        run_cli_command(
            query_history_args(&self.welink_cli_path, &self.group_id, cursor),
            self.request_timeout,
        )
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(e.to_string()))
    }

    async fn send_html(&self, html: &str) -> Result<SentMessageId, ChatChannelError> {
        let chunks = split_message(html, MAX_CONTENT_CHARS);
        let mut last_id = String::new();

        for chunk in chunks {
            let body = serde_json::json!({
                "content": chunk,
                "receiver": self.group_id,
                "auth": self.token,
            });
            let resp = self
                .client
                .post(&self.send_http_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChatChannelError::SendFailed(e.to_string()))?;
            let status = resp.status();
            let text = resp
                .text()
                .await
                .map_err(|e| ChatChannelError::SendFailed(e.to_string()))?;
            if !status.is_success() {
                return Err(ChatChannelError::SendFailed(format!(
                    "HTTP {status}: {}",
                    preview_text(&text, 300)
                )));
            }
            if !text.trim().is_empty() {
                last_id = text;
            }
        }

        Ok(SentMessageId(last_id))
    }
}

#[async_trait]
impl ChatChannelBackend for WelinkBackend {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Welink
    }

    async fn start(
        &self,
        command_tx: mpsc::Sender<IncomingCommand>,
    ) -> Result<(), ChatChannelError> {
        *self.status.lock().await = ChannelConnectionStatus::Connecting;
        if let Err(e) = self.run_auth_login().await {
            *self.status.lock().await = ChannelConnectionStatus::Error;
            return Err(e);
        }

        let initial_output = match self.query_history(None).await {
            Ok(output) => output,
            Err(e) => {
                *self.status.lock().await = ChannelConnectionStatus::Error;
                return Err(e);
            }
        };
        let initial = parse_history_output(
            &initial_output,
            None,
            &self.include_sender,
            &self.exclude_sender,
            true,
        )?;
        let mut cursor = initial.next_cursor;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        *self.shutdown_tx.lock().await = Some(shutdown_tx);
        *self.status.lock().await = ChannelConnectionStatus::Connected;

        let channel_id = self.channel_id;
        let group_id = self.group_id.clone();
        let welink_cli_path = self.welink_cli_path.clone();
        let include_sender = self.include_sender.clone();
        let exclude_sender = self.exclude_sender.clone();
        let request_timeout = self.request_timeout;
        let status = self.status.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)) => {}
                    _ = shutdown_rx.changed() => break,
                }

                let output = run_cli_command(
                    query_history_args(&welink_cli_path, &group_id, cursor.as_deref()),
                    request_timeout,
                )
                .await;

                match output {
                    Ok(stdout) => {
                        match parse_history_output(
                            &stdout,
                            cursor.as_deref(),
                            &include_sender,
                            &exclude_sender,
                            false,
                        ) {
                            Ok(batch) => {
                                cursor = batch.next_cursor;
                                *status.lock().await = ChannelConnectionStatus::Connected;
                                for msg in batch.messages {
                                    if let Err(e) = command_tx
                                        .send(IncomingCommand {
                                            channel_id,
                                            sender_id: msg.sender,
                                            command_text: msg.text,
                                            metadata: msg.metadata,
                                        })
                                        .await
                                    {
                                        eprintln!("[WeLink] command_tx.send failed: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[WeLink] history parse failed: {e}");
                                *status.lock().await = ChannelConnectionStatus::Error;
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)) => {}
                                    _ = shutdown_rx.changed() => break,
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[WeLink] history query failed: {e}");
                        *status.lock().await = ChannelConnectionStatus::Error;
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)) => {}
                            _ = shutdown_rx.changed() => break,
                        }
                    }
                }
            }

            *status.lock().await = ChannelConnectionStatus::Disconnected;
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ChatChannelError> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(true);
        }
        *self.status.lock().await = ChannelConnectionStatus::Disconnected;
        Ok(())
    }

    async fn status(&self) -> ChannelConnectionStatus {
        *self.status.lock().await
    }

    async fn send_message(&self, text: &str) -> Result<SentMessageId, ChatChannelError> {
        self.send_html(&html_escape_text(text)).await
    }

    async fn send_rich_message(
        &self,
        message: &RichMessage,
    ) -> Result<SentMessageId, ChatChannelError> {
        self.send_html(&format_welink_message(message)).await
    }

    async fn test_connection(&self) -> Result<(), ChatChannelError> {
        self.run_auth_login().await?;
        let output = self.query_history(None).await?;
        parse_history_output(
            &output,
            None,
            &self.include_sender,
            &self.exclude_sender,
            true,
        )?;
        Ok(())
    }
}

#[derive(Debug)]
struct CommandOutputError {
    words: Vec<String>,
    detail: String,
}

impl std::fmt::Display for CommandOutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", command_words_preview(&self.words), self.detail)
    }
}

fn auth_login_args(welink_cli_path: &str) -> Vec<String> {
    vec![
        welink_cli_path.to_string(),
        "auth".to_string(),
        "login".to_string(),
    ]
}

fn query_history_args(welink_cli_path: &str, group_id: &str, cursor: Option<&str>) -> Vec<String> {
    let mut words = vec![
        welink_cli_path.to_string(),
        "im".to_string(),
        "query-history-message".to_string(),
        "--group-id".to_string(),
        group_id.to_string(),
    ];
    if let Some(cursor) = cursor.filter(|c| !c.trim().is_empty()) {
        words.extend([
            "--message-id".to_string(),
            cursor.to_string(),
            "--query-direction".to_string(),
            "1".to_string(),
        ]);
    }
    words.extend(["--query-count".to_string(), QUERY_COUNT.to_string()]);
    words
}

fn normalize_sender_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

async fn run_cli_command(
    words: Vec<String>,
    timeout: Duration,
) -> Result<String, CommandOutputError> {
    if words.is_empty() {
        return Err(CommandOutputError {
            words,
            detail: "command is empty".to_string(),
        });
    }

    let mut command = Command::new(&words[0]);
    command.args(&words[1..]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| CommandOutputError {
            words: words.clone(),
            detail: format!("timed out after {} ms", timeout.as_millis()),
        })?
        .map_err(|e| CommandOutputError {
            words: words.clone(),
            detail: format!("failed to run command: {e}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(CommandOutputError {
            words,
            detail: format!(
                "exited with status {} stdout={} stderr={}",
                output.status,
                preview_text(&stdout, 300),
                preview_text(&stderr, 300)
            ),
        });
    }

    Ok(stdout)
}

#[derive(Debug, PartialEq)]
struct ParsedIncomingMessage {
    sender: String,
    text: String,
    metadata: serde_json::Value,
}

#[derive(Debug, PartialEq)]
struct ParsedHistoryBatch {
    messages: Vec<ParsedIncomingMessage>,
    next_cursor: Option<String>,
}

fn parse_history_output(
    output: &str,
    cursor: Option<&str>,
    include_sender: &[String],
    exclude_sender: &[String],
    initial_sync: bool,
) -> Result<ParsedHistoryBatch, ChatChannelError> {
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("Invalid JSON: {e}")))?;

    let result_code = value
        .get("resultCode")
        .and_then(value_to_string)
        .unwrap_or_else(|| "0".to_string());
    if result_code != "0" {
        let context = value
            .get("resultContext")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Err(ChatChannelError::ConnectionFailed(format!(
            "WeLink history query failed: code={result_code} context={context}"
        )));
    }

    let previous_cursor = cursor.and_then(|c| c.parse::<u64>().ok());
    let response_max_id = value.pointer("/respData/maxMsgId").and_then(value_to_u64);
    let mut messages = value
        .pointer("/respData/chatInfo")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    messages.sort_by_key(|message| json_u64(message, "msgId").unwrap_or(0));

    let batch_max_id = messages
        .iter()
        .filter_map(|message| json_u64(message, "msgId"))
        .max();
    let next_cursor = [previous_cursor, response_max_id, batch_max_id]
        .into_iter()
        .flatten()
        .max()
        .map(|id| id.to_string());

    if initial_sync {
        return Ok(ParsedHistoryBatch {
            messages: Vec::new(),
            next_cursor,
        });
    }

    let lower_bound = previous_cursor.unwrap_or(0);
    let parsed = messages
        .into_iter()
        .filter(|message| json_u64(message, "msgId").is_none_or(|id| id > lower_bound))
        .filter_map(|message| history_message_to_incoming(message, include_sender, exclude_sender))
        .collect();

    Ok(ParsedHistoryBatch {
        messages: parsed,
        next_cursor,
    })
}

fn history_message_to_incoming(
    message: serde_json::Value,
    include_sender: &[String],
    exclude_sender: &[String],
) -> Option<ParsedIncomingMessage> {
    let sender = message
        .get("sender")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("welink-user")
        .to_string();
    if !sender_allowed(&sender, include_sender, exclude_sender) {
        return None;
    }

    let text = extract_message_text(&message)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    Some(ParsedIncomingMessage {
        sender,
        text,
        metadata: message,
    })
}

fn sender_allowed(sender: &str, include_sender: &[String], exclude_sender: &[String]) -> bool {
    if exclude_sender.iter().any(|s| s == sender) {
        return false;
    }
    include_sender.is_empty() || include_sender.iter().any(|s| s == sender)
}

fn extract_message_text(message: &serde_json::Value) -> Option<String> {
    match message
        .get("contentType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "TEXT_MSG" => message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(html_to_text),
        "CARD_MSG" => message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
            .and_then(|card| {
                card.pointer("/cardContext/replyMsg").and_then(|reply| {
                    value_text(reply, &["content", "text", "PcContent"]).map(|v| html_to_text(&v))
                })
            }),
        _ => None,
    }
}

fn value_text(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

fn value_to_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|v| v.to_string()))
        .or_else(|| value.as_u64().map(|v| v.to_string()))
}

fn value_to_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn json_u64(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(value_to_u64)
}

fn format_welink_message(message: &RichMessage) -> String {
    let mut text = String::new();
    if let Some(title) = &message.title {
        text.push_str("<b>");
        text.push_str(&html_escape_text(title));
        text.push_str("</b>\n");
    }
    text.push_str(&html_escape_text(&message.body));
    for (key, value) in &message.fields {
        text.push('\n');
        text.push_str("<b>");
        text.push_str(&html_escape_text(key));
        text.push_str("</b>: ");
        text.push_str(&html_escape_text(value));
    }
    text
}

fn split_message(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in line_segments(text) {
        let mut remaining = line.as_str();
        while remaining.chars().count() > limit {
            let (head, tail) = split_at_char_limit(remaining, limit);
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.push(head.to_string());
            remaining = tail;
        }

        if current.chars().count() + remaining.chars().count() > limit && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(remaining);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn line_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            segments.push(text[start..index + ch.len_utf8()].to_string());
            start = index + ch.len_utf8();
        }
    }
    if start < text.len() {
        segments.push(text[start..].to_string());
    }
    segments
}

fn split_at_char_limit(value: &str, limit: usize) -> (&str, &str) {
    let split = value
        .char_indices()
        .nth(limit)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value.split_at(split)
}

fn html_to_text(value: &str) -> String {
    html_unescape(&strip_html_tags(&html_unescape(value)))
}

fn strip_html_tags(value: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
}

fn html_escape_text(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn preview_text(value: &str, limit: usize) -> String {
    let sanitized = value
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    let mut preview = sanitized.chars().take(limit).collect::<String>();
    if sanitized.chars().count() > limit {
        preview.push_str("...");
    }
    preview
}

fn command_words_preview(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            if word.is_empty() || word.chars().any(char::is_whitespace) {
                format!("{word:?}")
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_history(messages: Vec<serde_json::Value>, max_msg_id: u64) -> String {
        serde_json::json!({
            "resultCode": "0",
            "respData": {
                "maxMsgId": max_msg_id,
                "chatInfo": messages,
            }
        })
        .to_string()
    }

    #[test]
    fn config_deserializes_defaults() {
        let cfg: WelinkConfig = serde_json::from_value(serde_json::json!({
            "group_id": "group-1",
            "send_http_url": "http://example.invalid/"
        }))
        .expect("config");

        assert_eq!(cfg.group_id, "group-1");
        assert_eq!(cfg.send_http_url, "http://example.invalid/");
        assert_eq!(cfg.welink_cli_path, "welink-cli");
        assert!(cfg.include_sender.is_empty());
        assert!(cfg.exclude_sender.is_empty());
        assert_eq!(cfg.request_timeout_ms, 10_000);
    }

    #[test]
    fn builds_fixed_query_history_args_without_cursor() {
        assert_eq!(
            query_history_args("C:/WeLink/welink-cli.exe", "group-1", None),
            vec![
                "C:/WeLink/welink-cli.exe",
                "im",
                "query-history-message",
                "--group-id",
                "group-1",
                "--query-count",
                "100",
            ]
        );
    }

    #[test]
    fn builds_fixed_query_history_args_with_cursor() {
        assert_eq!(
            query_history_args("welink-cli", "group-1", Some("42")),
            vec![
                "welink-cli",
                "im",
                "query-history-message",
                "--group-id",
                "group-1",
                "--message-id",
                "42",
                "--query-direction",
                "1",
                "--query-count",
                "100",
            ]
        );
    }

    #[test]
    fn sender_filter_exclude_takes_precedence() {
        let include = vec!["alice".to_string()];
        let exclude = vec!["alice".to_string()];
        assert!(!sender_allowed("alice", &include, &exclude));
        assert!(!sender_allowed("bob", &include, &[]));
        assert!(sender_allowed("bob", &[], &[]));
    }

    #[test]
    fn initial_sync_records_cursor_without_messages() {
        let output = sample_history(
            vec![serde_json::json!({
                "msgId": 10,
                "sender": "alice",
                "contentType": "TEXT_MSG",
                "content": "/help"
            })],
            12,
        );
        let batch = parse_history_output(&output, None, &[], &[], true).expect("parse");
        assert!(batch.messages.is_empty());
        assert_eq!(batch.next_cursor.as_deref(), Some("12"));
    }

    #[test]
    fn parses_new_text_messages_and_ignores_old_messages() {
        let output = sample_history(
            vec![
                serde_json::json!({
                    "msgId": 4,
                    "sender": "old",
                    "contentType": "TEXT_MSG",
                    "content": "/old"
                }),
                serde_json::json!({
                    "msgId": 6,
                    "sender": "alice",
                    "contentType": "TEXT_MSG",
                    "content": "&lt;b&gt;/status&lt;/b&gt;"
                }),
            ],
            7,
        );
        let batch = parse_history_output(&output, Some("5"), &[], &[], false).expect("parse");
        assert_eq!(batch.next_cursor.as_deref(), Some("7"));
        assert_eq!(batch.messages.len(), 1);
        assert_eq!(batch.messages[0].sender, "alice");
        assert_eq!(batch.messages[0].text, "/status");
    }

    #[test]
    fn include_and_exclude_sender_filter_history_messages() {
        let output = sample_history(
            vec![
                serde_json::json!({
                    "msgId": 6,
                    "sender": "alice",
                    "contentType": "TEXT_MSG",
                    "content": "/status"
                }),
                serde_json::json!({
                    "msgId": 7,
                    "sender": "bob",
                    "contentType": "TEXT_MSG",
                    "content": "/help"
                }),
            ],
            7,
        );
        let include = vec!["alice".to_string(), "bob".to_string()];
        let exclude = vec!["bob".to_string()];
        let batch =
            parse_history_output(&output, Some("5"), &include, &exclude, false).expect("parse");
        assert_eq!(batch.messages.len(), 1);
        assert_eq!(batch.messages[0].sender, "alice");
    }

    #[test]
    fn extracts_card_reply_text() {
        let card = serde_json::json!({
            "cardContext": {
                "replyMsg": {
                    "content": "&lt;i&gt;approve&lt;/i&gt;"
                }
            }
        });
        let message = serde_json::json!({
            "msgId": 8,
            "sender": "alice",
            "contentType": "CARD_MSG",
            "content": card.to_string()
        });
        assert_eq!(extract_message_text(&message).as_deref(), Some("approve"));
    }

    #[test]
    fn splits_messages_at_welink_limit() {
        let body = "a".repeat(MAX_CONTENT_CHARS + 20);
        let chunks = split_message(&body, MAX_CONTENT_CHARS);
        assert_eq!(chunks.len(), 2);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= MAX_CONTENT_CHARS));
    }
}
