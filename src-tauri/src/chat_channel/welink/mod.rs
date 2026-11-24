pub mod channel;
pub mod protocol;
pub mod register;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch, Mutex};

use self::channel::WelinkShared;
use crate::chat_channel::error::ChatChannelError;
use crate::chat_channel::traits::ChatChannelBackend;
use crate::chat_channel::types::{
    ChannelConnectionStatus, ChannelMessageTarget, ChannelType, IncomingCommand, RichMessage,
    SentMessageId, WelinkConfig,
};

pub struct WelinkBackend {
    shared: Arc<WelinkShared>,
    shutdown_tx: Mutex<Option<watch::Sender<bool>>>,
}

impl WelinkBackend {
    pub fn new(channel_id: i32, config: WelinkConfig, sk: String) -> Self {
        Self {
            shared: Arc::new(WelinkShared::new(channel_id, config, sk)),
            shutdown_tx: Mutex::new(None),
        }
    }

    async fn send_target_message(
        &self,
        message: &RichMessage,
        target: &ChannelMessageTarget,
        finish_turn: bool,
    ) -> Result<SentMessageId, ChatChannelError> {
        let payload = target.provider_payload.as_ref();
        let tool_session_id = payload
            .and_then(|p| p.get("toolSessionId"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| target.chat_id.as_deref().filter(|s| !s.trim().is_empty()))
            .ok_or_else(|| {
                ChatChannelError::SendFailed("WeLink target missing toolSessionId".into())
            })?;

        let welink_session_id = payload
            .and_then(|p| p.get("welinkSessionId"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());

        channel::send_turn_message(
            &self.shared,
            tool_session_id,
            welink_session_id,
            &message.to_plain_text(),
            finish_turn,
        )
        .await
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
        // Stop any previous loop before starting a fresh one.
        if let Some(prev) = self.shutdown_tx.lock().await.take() {
            let _ = prev.send(true);
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        *self.shutdown_tx.lock().await = Some(shutdown_tx);
        *self.shared.status.lock().await = ChannelConnectionStatus::Connecting;

        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            channel::run_ws_loop(shared, command_tx, shutdown_rx).await;
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ChatChannelError> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(true);
        }
        *self.shared.outbound.lock().await = None;
        *self.shared.status.lock().await = ChannelConnectionStatus::Disconnected;
        Ok(())
    }

    async fn status(&self) -> ChannelConnectionStatus {
        *self.shared.status.lock().await
    }

    async fn send_message(&self, _text: &str) -> Result<SentMessageId, ChatChannelError> {
        // WeLink requires an active session `toolSessionId` to route upstream events;
        // broadcast/targetless sends are unsupported in stage 1.
        tracing::debug!(
            "[WeLink] channel={} ignoring targetless send_message",
            self.shared.channel_id
        );
        Err(ChatChannelError::Unsupported(
            "WeLink requires a session target (toolSessionId) to send messages".into(),
        ))
    }

    async fn send_rich_message(
        &self,
        _message: &RichMessage,
    ) -> Result<SentMessageId, ChatChannelError> {
        // WeLink requires an active session `toolSessionId` to route upstream events;
        // broadcast/targetless sends are unsupported in stage 1.
        tracing::debug!(
            "[WeLink] channel={} ignoring targetless send_rich_message",
            self.shared.channel_id
        );
        Err(ChatChannelError::Unsupported(
            "WeLink requires a session target (toolSessionId) to send messages".into(),
        ))
    }

    async fn send_rich_message_to(
        &self,
        message: &RichMessage,
        target: &ChannelMessageTarget,
    ) -> Result<SentMessageId, ChatChannelError> {
        self.send_target_message(message, target, true).await
    }

    async fn send_turn_update(
        &self,
        message: &RichMessage,
        target: &ChannelMessageTarget,
    ) -> Result<SentMessageId, ChatChannelError> {
        self.send_target_message(message, target, false).await
    }

    async fn test_connection(&self) -> Result<(), ChatChannelError> {
        channel::test_ws_connection(&self.shared).await
    }
}
