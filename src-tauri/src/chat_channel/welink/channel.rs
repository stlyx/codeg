use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};

use super::protocol::{
    build_auth_subprotocol, classify_close_code, generate_message_id, generate_tool_session_id,
    get_device_hostname, normalize_gateway_ws_url, parse_downstream, ChatPayload, CloseAction,
    CreateSessionPayload, DownstreamMessage, InvokeEnvelope, SessionIdPayload, UpstreamMessage,
    HANDSHAKE_TIMEOUT, HEARTBEAT_INTERVAL, SUPPRESS_REPLY_TEXT, TOOL_DONE_COMPAT_DELAY,
};
use super::register::get_first_mac;
use crate::chat_channel::error::ChatChannelError;
use crate::chat_channel::types::{
    ChannelConnectionStatus, ChannelMessageTarget, IncomingCommand, SentMessageId, WelinkConfig,
};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Number of fast exponential-backoff attempts before switching to 5-minute slow retries.
const MAX_FAST_RETRIES: u32 = 10;
const MAX_FAST_BACKOFF_SECS: u64 = 30;
const SLOW_RETRY_SECS: u64 = 300;
const OUTBOUND_BUFFER: usize = 128;

pub struct WelinkSession {
    pub welink_session_id: String,
    pub sender_id: String,
    #[allow(dead_code)]
    pub message_id_seq: AtomicU64,
}

pub enum OutboundFrame {
    Json(UpstreamMessage),
    Pong(tungstenite::Bytes),
    Close(Option<CloseFrame>),
}

pub struct WelinkShared {
    pub channel_id: i32,
    pub config: WelinkConfig,
    pub sk: String,
    pub status: Mutex<ChannelConnectionStatus>,
    pub last_error: Mutex<Option<String>>,
    pub sessions: Mutex<HashMap<String, WelinkSession>>,
    pub outbound: Mutex<Option<mpsc::Sender<OutboundFrame>>>,
    /// Serializes multi-frame reply sequences (`step.start` -> `text.done` -> `step.done` -> `tool_done`)
    /// so concurrent replies cannot interleave frames on the shared WebSocket.
    pub turn_send_lock: Mutex<()>,
}

impl WelinkShared {
    pub fn new(channel_id: i32, config: WelinkConfig, sk: String) -> Self {
        Self {
            channel_id,
            config,
            sk,
            status: Mutex::new(ChannelConnectionStatus::Disconnected),
            last_error: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            outbound: Mutex::new(None),
            turn_send_lock: Mutex::new(()),
        }
    }

    pub fn build_register_message(&self) -> UpstreamMessage {
        let device_name = self
            .config
            .device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(get_device_hostname);
        UpstreamMessage::register(
            device_name,
            self.config.assistant_name.clone(),
            get_first_mac(),
        )
    }
}

enum HandshakeOutcome {
    Connected(Box<WsStream>),
    Rejected(String),
    TerminalClose(u16, String),
    TransportError { message: String, slept: bool },
}

async fn open_and_register(shared: &WelinkShared) -> HandshakeOutcome {
    let ws_url = match normalize_gateway_ws_url(&shared.config.gateway_url) {
        Ok(url) => url,
        Err(e) => return HandshakeOutcome::Rejected(e.to_string()),
    };

    let subprotocol = build_auth_subprotocol(&shared.config.ak, &shared.sk);
    let mut req = match ws_url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => {
            return HandshakeOutcome::TransportError {
                message: format!("Failed to build WebSocket request: {e}"),
                slept: false,
            }
        }
    };
    match subprotocol.parse() {
        Ok(header_val) => {
            req.headers_mut()
                .insert("Sec-WebSocket-Protocol", header_val);
        }
        Err(e) => {
            return HandshakeOutcome::Rejected(format!("Invalid auth subprotocol header: {e}"));
        }
    }

    let started = Instant::now();
    let connect_result = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::connect_async(req),
    )
    .await;

    let mut ws_stream = match connect_result {
        Ok(Ok((stream, _resp))) => stream,
        Ok(Err(e)) => {
            return HandshakeOutcome::TransportError {
                message: format!("WebSocket connect failed: {e}"),
                slept: false,
            };
        }
        Err(_) => {
            let slept = started.elapsed() > HANDSHAKE_TIMEOUT * 3;
            return HandshakeOutcome::TransportError {
                message: "WebSocket connect timed out".into(),
                slept,
            };
        }
    };

    let register_json = match shared.build_register_message().to_json_string() {
        Ok(j) => j,
        Err(e) => {
            return HandshakeOutcome::TransportError {
                message: format!("Failed to serialize register frame: {e}"),
                slept: false,
            }
        }
    };

    if let Err(e) = ws_stream
        .send(tungstenite::Message::Text(register_json.into()))
        .await
    {
        return HandshakeOutcome::TransportError {
            message: format!("Failed to send register frame: {e}"),
            slept: false,
        };
    }

    // Wait for `register_ok` or `register_rejected` within HANDSHAKE_TIMEOUT.
    let handshake_started = Instant::now();
    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let slept = handshake_started.elapsed() > HANDSHAKE_TIMEOUT * 3;
            return HandshakeOutcome::TransportError {
                message: "register handshake timed out".into(),
                slept,
            };
        }

        match tokio::time::timeout(remaining, ws_stream.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                match parse_downstream(text.as_str()) {
                    Ok(DownstreamMessage::RegisterOk) => {
                        return HandshakeOutcome::Connected(Box::new(ws_stream));
                    }
                    Ok(DownstreamMessage::RegisterRejected { reason }) => {
                        let _ = ws_stream.close(None).await;
                        return HandshakeOutcome::Rejected(reason);
                    }
                    Ok(other) => {
                        tracing::debug!(
                            "[WeLink] channel={} ignoring pre-register frame: {:?}",
                            shared.channel_id,
                            other
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[WeLink] channel={} invalid JSON during register handshake: {e}",
                            shared.channel_id
                        );
                    }
                }
            }
            Ok(Some(Ok(tungstenite::Message::Ping(data)))) => {
                let _ = ws_stream.send(tungstenite::Message::Pong(data)).await;
            }
            Ok(Some(Ok(tungstenite::Message::Close(frame)))) => {
                let code = frame
                    .as_ref()
                    .map(|f| u16::from(f.code))
                    .unwrap_or(1006);
                let reason = frame
                    .as_ref()
                    .map(|f| f.reason.to_string())
                    .unwrap_or_default();
                if classify_close_code(code) == CloseAction::Terminal {
                    return HandshakeOutcome::TerminalClose(code, reason);
                }
                return HandshakeOutcome::TransportError {
                    message: format!("WebSocket closed during register (code={code}, reason={reason})"),
                    slept: false,
                };
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => {
                return HandshakeOutcome::TransportError {
                    message: format!("WebSocket error during register: {e}"),
                    slept: false,
                };
            }
            Ok(None) => {
                return HandshakeOutcome::TransportError {
                    message: "WebSocket stream ended during register".into(),
                    slept: false,
                };
            }
            Err(_) => {
                let slept = handshake_started.elapsed() > HANDSHAKE_TIMEOUT * 3;
                return HandshakeOutcome::TransportError {
                    message: "register handshake timed out".into(),
                    slept,
                };
            }
        }
    }
}

/// One-shot connection test: connect -> register -> expect `register_ok` -> close(1000).
pub async fn test_ws_connection(shared: &WelinkShared) -> Result<(), ChatChannelError> {
    match open_and_register(shared).await {
        HandshakeOutcome::Connected(mut ws_stream) => {
            let close_frame = CloseFrame {
                code: CloseCode::Normal,
                reason: "test_connection complete".into(),
            };
            let _ = (*ws_stream).close(Some(close_frame)).await;
            Ok(())
        }
        HandshakeOutcome::Rejected(reason) => {
            Err(ChatChannelError::AuthenticationFailed(reason))
        }
        HandshakeOutcome::TerminalClose(code, reason) => Err(
            ChatChannelError::AuthenticationFailed(format!("gateway closed ({code}): {reason}")),
        ),
        HandshakeOutcome::TransportError { message, .. } => {
            Err(ChatChannelError::ConnectionFailed(message))
        }
    }
}

fn compute_reconnect_delay(retry_count: u32) -> Duration {
    if retry_count >= MAX_FAST_RETRIES {
        return Duration::from_secs(SLOW_RETRY_SECS);
    }
    let exp_secs = 1u64.checked_shl(retry_count.min(5)).unwrap_or(MAX_FAST_BACKOFF_SECS);
    let capped_ms = exp_secs.min(MAX_FAST_BACKOFF_SECS) * 1000;
    let jitter_ms = rand::thread_rng().gen_range(0..=capped_ms);
    Duration::from_millis(jitter_ms.max(100))
}

/// Long-lived WebSocket connection & reconnect loop spawned by `WelinkBackend::start()`.
pub async fn run_ws_loop(
    shared: Arc<WelinkShared>,
    command_tx: mpsc::Sender<IncomingCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut retry_count = 0u32;

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        *shared.status.lock().await = ChannelConnectionStatus::Connecting;

        let ws_stream = match open_and_register(&shared).await {
            HandshakeOutcome::Connected(stream) => {
                *shared.status.lock().await = ChannelConnectionStatus::Connected;
                *shared.last_error.lock().await = None;
                retry_count = 0;
                tracing::info!(
                    "[WeLink] channel={} connected and registered",
                    shared.channel_id
                );
                *stream
            }
            HandshakeOutcome::Rejected(reason) => {
                tracing::error!(
                    "[WeLink] channel={} register rejected (terminal): {reason}",
                    shared.channel_id
                );
                *shared.last_error.lock().await = Some(reason);
                *shared.status.lock().await = ChannelConnectionStatus::Error;
                return;
            }
            HandshakeOutcome::TerminalClose(code, reason) => {
                let msg = format!("gateway closed with terminal code {code}: {reason}");
                tracing::error!("[WeLink] channel={} {msg}", shared.channel_id);
                *shared.last_error.lock().await = Some(msg);
                *shared.status.lock().await = ChannelConnectionStatus::Error;
                return;
            }
            HandshakeOutcome::TransportError { message, slept } => {
                if slept {
                    tracing::info!(
                        "[WeLink] channel={} detected system sleep/wake during handshake; resetting retry window",
                        shared.channel_id
                    );
                    retry_count = 0;
                } else {
                    tracing::warn!(
                        "[WeLink] channel={} handshake failed (attempt={}): {message}",
                        shared.channel_id,
                        retry_count + 1
                    );
                    retry_count = retry_count.saturating_add(1);
                }
                *shared.last_error.lock().await = Some(message);
                *shared.status.lock().await = ChannelConnectionStatus::Error;
                let delay = compute_reconnect_delay(retry_count.saturating_sub(1));
                tokio::select! {
                    _ = tokio::time::sleep(delay) => continue,
                    _ = shutdown_rx.changed() => break,
                }
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundFrame>(OUTBOUND_BUFFER);
        *shared.outbound.lock().await = Some(outbound_tx.clone());

        // Dedicated writer task
        let write_channel_id = shared.channel_id;
        let writer_handle = tokio::spawn(async move {
            while let Some(frame) = outbound_rx.recv().await {
                let ws_msg = match frame {
                    OutboundFrame::Json(msg) => match msg.to_json_string() {
                        Ok(json) => tungstenite::Message::Text(json.into()),
                        Err(e) => {
                            tracing::error!(
                                "[WeLink] channel={write_channel_id} serialize error: {e}"
                            );
                            continue;
                        }
                    },
                    OutboundFrame::Pong(data) => tungstenite::Message::Pong(data),
                    OutboundFrame::Close(frame) => tungstenite::Message::Close(frame),
                };
                if let Err(e) = ws_write.send(ws_msg).await {
                    tracing::warn!(
                        "[WeLink] channel={write_channel_id} write failed: {e}"
                    );
                    break;
                }
            }
        });

        // 30s application-layer heartbeat task
        let hb_tx = outbound_tx.clone();
        let mut hb_shutdown = shutdown_rx.clone();
        let heartbeat_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
            // Skip immediate first tick right after register
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if hb_tx.send(OutboundFrame::Json(UpstreamMessage::heartbeat())).await.is_err() {
                            break;
                        }
                    }
                    _ = hb_shutdown.changed() => break,
                }
            }
        });

        let mut terminal_exit = false;
        loop {
            tokio::select! {
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(tungstenite::Message::Text(text))) => {
                            match parse_downstream(text.as_str()) {
                                Ok(parsed) => {
                                    handle_downstream_message(
                                        &shared,
                                        parsed,
                                        &outbound_tx,
                                        &command_tx,
                                    ).await;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "[WeLink] channel={} failed to parse downstream message: {e}",
                                        shared.channel_id
                                    );
                                }
                            }
                        }
                        Some(Ok(tungstenite::Message::Ping(data))) => {
                            let _ = outbound_tx.send(OutboundFrame::Pong(data)).await;
                        }
                        Some(Ok(tungstenite::Message::Close(frame))) => {
                            let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(1006);
                            let reason = frame.as_ref().map(|f| f.reason.to_string()).unwrap_or_default();
                            match classify_close_code(code) {
                                CloseAction::Terminal => {
                                    let msg = format!("gateway closed with terminal code {code}: {reason}");
                                    tracing::error!("[WeLink] channel={} {msg}", shared.channel_id);
                                    *shared.last_error.lock().await = Some(msg);
                                    *shared.status.lock().await = ChannelConnectionStatus::Error;
                                    terminal_exit = true;
                                }
                                CloseAction::Reconnect => {
                                    tracing::info!(
                                        "[WeLink] channel={} WebSocket closed (code={code}), reconnecting",
                                        shared.channel_id
                                    );
                                }
                                CloseAction::ReconnectWithWarn => {
                                    tracing::warn!(
                                        "[WeLink] channel={} WebSocket closed (code={code}, reason={reason}), reconnecting",
                                        shared.channel_id
                                    );
                                }
                            }
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            tracing::warn!(
                                "[WeLink] channel={} WebSocket read error: {e}",
                                shared.channel_id
                            );
                            break;
                        }
                        None => {
                            tracing::info!(
                                "[WeLink] channel={} WebSocket stream closed, reconnecting",
                                shared.channel_id
                            );
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        let _ = outbound_tx.send(OutboundFrame::Close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "channel stopped".into(),
                        }))).await;
                        *shared.outbound.lock().await = None;
                        heartbeat_handle.abort();
                        drop(outbound_tx);
                        let _ = tokio::time::timeout(Duration::from_secs(2), writer_handle).await;
                        *shared.status.lock().await = ChannelConnectionStatus::Disconnected;
                        return;
                    }
                }
            }
        }

        // Tear down per-connection tasks while keeping `shared.sessions` intact across reconnects.
        *shared.outbound.lock().await = None;
        heartbeat_handle.abort();
        drop(outbound_tx);
        writer_handle.abort();

        if terminal_exit {
            return;
        }

        *shared.status.lock().await = ChannelConnectionStatus::Connecting;
        let delay = compute_reconnect_delay(retry_count);
        retry_count = retry_count.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown_rx.changed() => break,
        }
    }

    *shared.outbound.lock().await = None;
    *shared.status.lock().await = ChannelConnectionStatus::Disconnected;
}

async fn handle_downstream_message(
    shared: &Arc<WelinkShared>,
    msg: DownstreamMessage,
    outbound_tx: &mpsc::Sender<OutboundFrame>,
    command_tx: &mpsc::Sender<IncomingCommand>,
) {
    match msg {
        DownstreamMessage::StatusQuery => {
            let online = *shared.status.lock().await == ChannelConnectionStatus::Connected;
            let _ = outbound_tx
                .send(OutboundFrame::Json(UpstreamMessage::status_response(online)))
                .await;
        }
        DownstreamMessage::Invoke(env) => {
            handle_invoke(shared, *env, outbound_tx, command_tx).await;
        }
        DownstreamMessage::RegisterOk => {}
        DownstreamMessage::RegisterRejected { reason } => {
            tracing::error!(
                "[WeLink] channel={} unexpected register_rejected: {reason}",
                shared.channel_id
            );
        }
        DownstreamMessage::Unknown(val) => {
            tracing::debug!(
                "[WeLink] channel={} ignoring unknown downstream frame: {}",
                shared.channel_id,
                val
            );
        }
    }
}

async fn handle_invoke(
    shared: &Arc<WelinkShared>,
    env: InvokeEnvelope,
    outbound_tx: &mpsc::Sender<OutboundFrame>,
    command_tx: &mpsc::Sender<IncomingCommand>,
) {
    match env.action.as_str() {
        "create_session" => {
            let Some(welink_session_id) = env
                .welink_session_id
                .clone()
                .filter(|s| !s.trim().is_empty())
            else {
                let _ = outbound_tx
                    .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                        None,
                        None,
                        "missing welinkSessionId in create_session",
                        None,
                    )))
                    .await;
                return;
            };

            let payload: CreateSessionPayload = env
                .payload
                .clone()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();

            let tool_session_id = generate_tool_session_id();
            let sender_id = payload.business_session_id().unwrap_or_default().to_string();

            shared.sessions.lock().await.insert(
                tool_session_id.clone(),
                WelinkSession {
                    welink_session_id: welink_session_id.clone(),
                    sender_id,
                    message_id_seq: AtomicU64::new(0),
                },
            );

            tracing::info!(
                "[WeLink] channel={} create_session welinkSessionId={} -> toolSessionId={}",
                shared.channel_id,
                welink_session_id,
                tool_session_id
            );

            let _ = outbound_tx
                .send(OutboundFrame::Json(UpstreamMessage::session_created(
                    welink_session_id,
                    tool_session_id,
                    env.trace_id,
                    payload.title,
                )))
                .await;
        }
        "chat" => {
            let payload_val = match env.payload {
                Some(v) => v,
                None => {
                    let _ = outbound_tx
                        .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                            env.welink_session_id,
                            env.tool_session_id,
                            "missing chat payload",
                            None,
                        )))
                        .await;
                    return;
                }
            };
            let payload: ChatPayload = match serde_json::from_value(payload_val) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "[WeLink] channel={} invalid chat payload: {e}",
                        shared.channel_id
                    );
                    let _ = outbound_tx
                        .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                            env.welink_session_id,
                            env.tool_session_id,
                            "invalid chat payload",
                            None,
                        )))
                        .await;
                    return;
                }
            };

            let session_info = {
                let sessions = shared.sessions.lock().await;
                sessions.get(&payload.tool_session_id).map(|s| {
                    (s.welink_session_id.clone(), s.sender_id.clone())
                })
            };

            let Some((welink_session_id, fallback_sender)) = session_info else {
                tracing::warn!(
                    "[WeLink] channel={} chat for unknown toolSessionId={}",
                    shared.channel_id,
                    payload.tool_session_id
                );
                let _ = outbound_tx
                    .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                        None,
                        Some(payload.tool_session_id),
                        "session not found",
                        Some("session_not_found".to_string()),
                    )))
                    .await;
                return;
            };

            // `suppressReply: true` must NEVER reach the agent.
            if env.suppress_reply {
                tracing::info!(
                    "[WeLink] channel={} suppressReply=true for toolSessionId={}, sending synthetic notice",
                    shared.channel_id,
                    payload.tool_session_id
                );
                let shared_clone = Arc::clone(shared);
                let tsid = payload.tool_session_id;
                tokio::spawn(async move {
                    if let Err(e) = send_turn_message(
                        &shared_clone,
                        &tsid,
                        Some(&welink_session_id),
                        SUPPRESS_REPLY_TEXT,
                        true,
                    )
                    .await
                    {
                        tracing::warn!(
                            "[WeLink] channel={} failed to send suppressReply notice: {e}",
                            shared_clone.channel_id
                        );
                    }
                });
                return;
            }

            let sender_id = payload
                .send_user_account
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or(fallback_sender);

            let target = ChannelMessageTarget {
                channel_id: shared.channel_id,
                chat_id: Some(payload.tool_session_id.clone()),
                thread_key: None,
                thread_kind: None,
                provider_payload: Some(serde_json::json!({
                    "toolSessionId": payload.tool_session_id,
                    "welinkSessionId": welink_session_id,
                })),
            };

            let cmd = IncomingCommand {
                channel_id: shared.channel_id,
                sender_id,
                command_text: payload.text,
                callback_data: None,
                target,
                metadata: serde_json::json!({
                    "messageId": payload.message_id,
                }),
            };

            if let Err(e) = command_tx.send(cmd).await {
                tracing::error!(
                    "[WeLink] channel={} failed to forward chat to command_dispatcher: {e}",
                    shared.channel_id
                );
            }
        }
        "close_session" => {
            if let Some(payload) = env
                .payload
                .and_then(|v| serde_json::from_value::<SessionIdPayload>(v).ok())
            {
                shared.sessions.lock().await.remove(&payload.tool_session_id);
                tracing::info!(
                    "[WeLink] channel={} closed toolSessionId={}",
                    shared.channel_id,
                    payload.tool_session_id
                );
            }
        }
        "abort_session" => {
            if let Some(payload) = env
                .payload
                .and_then(|v| serde_json::from_value::<SessionIdPayload>(v).ok())
            {
                tracing::info!(
                    "[WeLink] channel={} received abort_session for toolSessionId={} (stage 1 no-op)",
                    shared.channel_id,
                    payload.tool_session_id
                );
            }
        }
        "permission_reply" | "question_reply" => {
            let tool_session_id = env.tool_session_id.or_else(|| {
                env.payload
                    .as_ref()
                    .and_then(|p| p.get("toolSessionId"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
            let _ = outbound_tx
                .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                    env.welink_session_id,
                    tool_session_id,
                    "unsupported action",
                    None,
                )))
                .await;
        }
        "query_slash_commands" => {
            let tool_session_id = env
                .tool_session_id
                .or_else(|| {
                    env.payload
                        .as_ref()
                        .and_then(|p| p.get("toolSessionId"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_default();
            let trace_id = env.trace_id.unwrap_or_default();
            let _ = outbound_tx
                .send(OutboundFrame::Json(UpstreamMessage::slash_commands_result(
                    tool_session_id,
                    trace_id,
                    Vec::new(),
                )))
                .await;
        }
        other => {
            tracing::warn!(
                "[WeLink] channel={} unsupported invoke action: {other}",
                shared.channel_id
            );
            let _ = outbound_tx
                .send(OutboundFrame::Json(UpstreamMessage::tool_error(
                    env.welink_session_id,
                    env.tool_session_id,
                    format!("unsupported action: {other}"),
                    None,
                )))
                .await;
        }
    }
}

/// Send one text message. Only the final message closes the gateway turn.
pub async fn send_turn_message(
    shared: &WelinkShared,
    tool_session_id: &str,
    welink_session_id: Option<&str>,
    text: &str,
    finish_turn: bool,
) -> Result<SentMessageId, ChatChannelError> {
    let _guard = shared.turn_send_lock.lock().await;

    let outbound_tx = shared
        .outbound
        .lock()
        .await
        .clone()
        .ok_or_else(|| ChatChannelError::SendFailed("WeLink WebSocket is not connected".into()))?;

    let message_id = generate_message_id();
    let part_id = format!("{message_id}-p0");

    let send_err = |_| ChatChannelError::SendFailed("WeLink WebSocket outbound channel closed".into());

    outbound_tx
        .send(OutboundFrame::Json(UpstreamMessage::step_start(
            tool_session_id.to_string(),
            &message_id,
        )))
        .await
        .map_err(send_err)?;

    outbound_tx
        .send(OutboundFrame::Json(UpstreamMessage::text_done(
            tool_session_id.to_string(),
            &message_id,
            &part_id,
            text,
        )))
        .await
        .map_err(send_err)?;

    outbound_tx
        .send(OutboundFrame::Json(UpstreamMessage::step_done(
            tool_session_id.to_string(),
            &message_id,
        )))
        .await
        .map_err(send_err)?;

    if finish_turn {
        tokio::time::sleep(TOOL_DONE_COMPAT_DELAY).await;

        outbound_tx
            .send(OutboundFrame::Json(UpstreamMessage::tool_done(
                tool_session_id.to_string(),
                welink_session_id.map(String::from),
            )))
            .await
            .map_err(send_err)?;
    }

    Ok(SentMessageId(message_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_channel::traits::ChatChannelBackend;
    use crate::chat_channel::types::RichMessage;
    use crate::chat_channel::welink::WelinkBackend;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    async fn spawn_mock_gateway<F, Fut>(handler: F) -> (String, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(WebSocketStream<TcpStream>, Option<String>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut captured_proto: Option<String> = None;
            #[allow(clippy::result_large_err)]
            let ws = tokio_tungstenite::accept_hdr_async(
                stream,
                |req: &Request, mut resp: Response| {
                    if let Some(val) = req.headers().get("Sec-WebSocket-Protocol") {
                        if let Ok(s) = val.to_str() {
                            captured_proto = Some(s.to_string());
                            resp.headers_mut()
                                .insert("Sec-WebSocket-Protocol", val.clone());
                        }
                    }
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            handler(ws, captured_proto).await;
        });
        (format!("127.0.0.1:{}", addr.port()), handle)
    }

    async fn read_json(ws: &mut WebSocketStream<TcpStream>) -> serde_json::Value {
        while let Some(msg) = ws.next().await {
            if let Ok(tungstenite::Message::Text(text)) = msg {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
        panic!("WebSocket closed before receiving expected JSON frame");
    }

    #[tokio::test]
    async fn full_ws_lifecycle_session_chat_reply_and_control_actions() {
        let (gateway_host, server_handle) = spawn_mock_gateway(|mut ws, proto| async move {
            let proto = proto.expect("Sec-WebSocket-Protocol must be set");
            assert!(proto.starts_with("auth."));

            // 1. Expect register
            let reg = read_json(&mut ws).await;
            assert_eq!(reg["type"], "register");
            assert_eq!(reg["toolType"], "codeagent");
            assert!(reg["toolVersion"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(reg["pluginVersion"].as_str().is_some_and(|s| !s.is_empty()));

            ws.send(tungstenite::Message::Text(r#"{"type":"register_ok"}"#.into()))
                .await
                .unwrap();

            // 2. status_query -> status_response
            ws.send(tungstenite::Message::Text(r#"{"type":"status_query"}"#.into()))
                .await
                .unwrap();
            let status_resp = read_json(&mut ws).await;
            assert_eq!(status_resp["type"], "status_response");
            assert_eq!(status_resp["opencodeOnline"], true);

            // 3. query_slash_commands -> echoes toolSessionId and traceId
            ws.send(tungstenite::Message::Text(
                r#"{"type":"invoke","action":"query_slash_commands","toolSessionId":"ts_slash","traceId":"tr_slash"}"#.into(),
            ))
            .await
            .unwrap();
            let slash_resp = read_json(&mut ws).await;
            assert_eq!(slash_resp["type"], "slash_commands_result");
            assert_eq!(slash_resp["toolSessionId"], "ts_slash");
            assert_eq!(slash_resp["traceId"], "tr_slash");
            assert_eq!(slash_resp["payload"]["slashCommands"], serde_json::json!([]));

            // 4. permission_reply -> tool_error("unsupported action")
            ws.send(tungstenite::Message::Text(
                r#"{"type":"invoke","action":"permission_reply","payload":{"permissionId":"p1","response":"once"}}"#.into(),
            ))
            .await
            .unwrap();
            let perm_err = read_json(&mut ws).await;
            assert_eq!(perm_err["type"], "tool_error");
            assert_eq!(perm_err["error"], "unsupported action");

            // 5. chat for unknown session -> session_not_found
            ws.send(tungstenite::Message::Text(
                r#"{"type":"invoke","action":"chat","payload":{"text":"hi","toolSessionId":"missing123"}}"#.into(),
            ))
            .await
            .unwrap();
            let nf = read_json(&mut ws).await;
            assert_eq!(nf["type"], "tool_error");
            assert_eq!(nf["toolSessionId"], "missing123");
            assert_eq!(nf["reason"], "session_not_found");

            // 6. create_session -> session_created (echoing traceId)
            ws.send(tungstenite::Message::Text(
                r#"{"type":"invoke","welinkSessionId":"wl_999","traceId":"tr_1","action":"create_session","payload":{"title":"Test","extParameters":{"platformExtParam":{"businessSessionId":"emp_01"}}}}"#.into(),
            ))
            .await
            .unwrap();
            let created = read_json(&mut ws).await;
            assert_eq!(created["type"], "session_created");
            assert_eq!(created["welinkSessionId"], "wl_999");
            assert_eq!(created["traceId"], "tr_1");
            let tool_sid = created["toolSessionId"].as_str().unwrap().to_string();
            assert_eq!(tool_sid.len(), 12);

            // 7. The paragraph arrives before the last message ends the turn.
            let chat_frame = serde_json::json!({
                "type": "invoke",
                "traceId": "tr_1",
                "action": "chat",
                "payload": {
                    "text": "hello agent",
                    "toolSessionId": tool_sid,
                    "sendUserAccount": "emp_01",
                    "messageId": "m_orig"
                }
            });
            ws.send(tungstenite::Message::Text(chat_frame.to_string().into()))
                .await
                .unwrap();

            let start = read_json(&mut ws).await;
            let paragraph = read_json(&mut ws).await;
            let done = read_json(&mut ws).await;
            assert_eq!(start["event"]["type"], "step.start");
            assert_eq!(paragraph["event"]["type"], "text.done");
            assert_eq!(paragraph["event"]["properties"]["content"], "first paragraph");
            assert_eq!(done["event"]["type"], "step.done");

            let f1 = read_json(&mut ws).await;
            let f2 = read_json(&mut ws).await;
            let f3 = read_json(&mut ws).await;
            let f4 = read_json(&mut ws).await;
            assert_eq!(f1["type"], "tool_event");
            assert_eq!(f1["event"]["type"], "step.start");
            assert_eq!(f1["event"]["protocol"], "cloud");
            let mid = f1["event"]["properties"]["messageId"].as_str().unwrap();

            assert_eq!(f2["type"], "tool_event");
            assert_eq!(f2["event"]["type"], "text.done");
            assert_eq!(f2["event"]["protocol"], "cloud");
            assert_eq!(f2["event"]["properties"]["messageId"], mid);
            assert_eq!(f2["event"]["properties"]["partId"], format!("{mid}-p0"));
            assert_eq!(f2["event"]["properties"]["content"], "reply body");

            assert_eq!(f3["type"], "tool_event");
            assert_eq!(f3["event"]["type"], "step.done");
            assert_eq!(f3["event"]["protocol"], "cloud");
            assert_eq!(f3["event"]["properties"]["messageId"], mid);

            assert_eq!(f4["type"], "tool_done");
            assert_eq!(f4["toolSessionId"], tool_sid);
            assert_eq!(f4["welinkSessionId"], "wl_999");
            assert!(f4.get("protocol").is_none());

            // 8. suppressReply=true -> synthetic turn, must NOT forward to agent
            let suppressed = serde_json::json!({
                "type": "invoke",
                "action": "chat",
                "suppressReply": true,
                "payload": {
                    "text": "@bot group question",
                    "toolSessionId": tool_sid,
                    "sendUserAccount": "emp_01"
                }
            });
            ws.send(tungstenite::Message::Text(suppressed.to_string().into()))
                .await
                .unwrap();
            let s1 = read_json(&mut ws).await;
            let s2 = read_json(&mut ws).await;
            let s3 = read_json(&mut ws).await;
            let s4 = read_json(&mut ws).await;
            assert_eq!(s1["event"]["type"], "step.start");
            assert_eq!(s2["event"]["type"], "text.done");
            assert_eq!(s2["event"]["properties"]["content"], SUPPRESS_REPLY_TEXT);
            assert_eq!(s3["event"]["type"], "step.done");
            assert_eq!(s4["type"], "tool_done");
        })
        .await;

        let cfg = WelinkConfig {
            assistant_name: "codeagent".into(),
            gateway_url: gateway_host,
            device_name: Some("test-host".into()),
            ak: "test-ak".into(),
        };
        let backend = WelinkBackend::new(42, cfg, "test-sk".into());
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        backend.start(cmd_tx).await.unwrap();

        let incoming = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for IncomingCommand")
            .expect("channel closed");
        assert_eq!(incoming.channel_id, 42);
        assert_eq!(incoming.sender_id, "emp_01");
        assert_eq!(incoming.command_text, "hello agent");

        backend
            .send_turn_update(&RichMessage::info("first paragraph"), &incoming.target)
            .await
            .unwrap();
        backend
            .send_rich_message_to(&RichMessage::info("reply body"), &incoming.target)
            .await
            .unwrap();

        server_handle.await.unwrap();

        // Ensure suppressed message was never forwarded to command_tx
        assert!(cmd_rx.try_recv().is_err());

        backend.stop().await.unwrap();
        assert_eq!(backend.status().await, ChannelConnectionStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_connection_handles_register_ok_and_rejected() {
        let (ok_host, ok_srv) = spawn_mock_gateway(|mut ws, _| async move {
            let _ = read_json(&mut ws).await;
            ws.send(tungstenite::Message::Text(r#"{"type":"register_ok"}"#.into()))
                .await
                .unwrap();
            // Drain close frame
            let _ = ws.next().await;
        })
        .await;

        let ok_backend = WelinkBackend::new(
            1,
            WelinkConfig {
                assistant_name: "codeagent".into(),
                gateway_url: ok_host,
                device_name: None,
                ak: "ak".into(),
            },
            "sk".into(),
        );
        ok_backend.test_connection().await.unwrap();
        ok_srv.await.unwrap();

        let (rej_host, rej_srv) = spawn_mock_gateway(|mut ws, _| async move {
            let _ = read_json(&mut ws).await;
            ws.send(tungstenite::Message::Text(
                r#"{"type":"register_rejected","reason":"invalid signature"}"#.into(),
            ))
            .await
            .unwrap();
        })
        .await;

        let rej_backend = WelinkBackend::new(
            2,
            WelinkConfig {
                assistant_name: "codeagent".into(),
                gateway_url: rej_host,
                device_name: None,
                ak: "ak".into(),
            },
            "bad-sk".into(),
        );
        let err = rej_backend.test_connection().await.unwrap_err();
        assert!(matches!(err, ChatChannelError::AuthenticationFailed(_)));
        assert!(err.to_string().contains("invalid signature"));
        rej_srv.await.unwrap();
    }
}
