//! Companion-side MCP protocol — the bits that live inside the `codeg-mcp`
//! binary but are factored out into the library so they can be unit-tested
//! without spawning the binary.
//!
//! The companion speaks newline-delimited JSON-RPC 2.0 on stdio:
//! one request → one response per line, with concurrent dispatch so
//! `notifications/cancelled` can race an in-flight `tools/call`. It exposes
//! exactly one tool — `delegate_to_agent` — whose schema is embedded at
//! compile time from [`TOOL_SCHEMA_JSON`].
//!
//! Notifications (id = None) produce no response, matching MCP's expectation
//! that `notifications/initialized` etc. are fire-and-forget.
//!
//! Cancellation flow per the MCP 2024-11-05 / 2025-11-25 cancellation utility:
//!
//! 1. Companion receives `tools/call` with JSON-RPC `id = X`, mints an opaque
//!    `external_handle`, registers `X → (handle, cancel_tx)` in
//!    [`InflightCalls`], and kicks off the broker round-trip.
//! 2. If `notifications/cancelled` for `requestId = X` arrives, the
//!    notification handler pops the entry, fires `cancel_tx`, and sends a
//!    `BrokerMessage::Cancel { external_handle }` to the broker.
//! 3. The `tools/call` task observes `cancel_tx`, abandons its UDS read,
//!    and returns `None` — the binary suppresses the response per spec.
//! 4. If the round-trip completes before the cancel arrives, the entry is
//!    removed normally and the response goes out on stdout; a late cancel
//!    notification finds nothing and is silently ignored.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use crate::acp::delegation::transport::{
    client_cancel, client_round_trip, BrokerCancelRequest, BrokerRequest,
};

/// Upper bound on one broker-side cancel round-trip. Bounds both
/// `handle_cancel_notification` (so stdin dispatch can't stall behind a
/// stuck UDS connect/read) and the shutdown-drain loop (so an
/// unresponsive listener can't keep the EOF / watchdog path hung). 500 ms
/// is generous for a same-host UDS exchange and short enough that a user
/// won't notice the bound being hit. Misses are absorbed by the codeg
/// main side's `cancel_by_parent` cascade when the parent ACP connection
/// eventually ends.
const BROKER_CANCEL_BUDGET: Duration = Duration::from_millis(500);

/// Wrap `client_cancel` in [`BROKER_CANCEL_BUDGET`] so callers can fire
/// a synchronous cancel without worrying about a hung listener freezing
/// them. Both success, transport error, and timeout collapse to `()` —
/// callers couldn't usefully react anyway, and the broker has independent
/// cancel backstops (parent / child disconnect cascades) if this one
/// misses.
async fn send_broker_cancel(socket_path: &str, req: &BrokerCancelRequest) {
    let _ = tokio::time::timeout(
        BROKER_CANCEL_BUDGET,
        client_cancel(socket_path, req),
    )
    .await;
}

/// Static MCP tool schema. Lives next to this module so codeg-mcp ships
/// a single embedded copy — no runtime file IO, no version skew with the
/// broker's [`super::types::DelegationRequest`].
pub const TOOL_SCHEMA_JSON: &str = include_str!("tool_schema.json");

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// MCP notifications carry no `id`. We dispatch a response only when this
    /// is `Some`.
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

pub fn ok(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(result),
        error: None,
    }
}

pub fn err(id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}

/// Process arguments threaded through every `tools/call` so the dispatcher
/// can build a [`BrokerRequest`] without re-parsing argv per call.
#[derive(Debug, Clone)]
pub struct CompanionContext {
    pub parent_connection_id: String,
    pub socket_path: String,
    pub token: String,
}

/// Per-in-flight-call state. The companion stashes one of these per
/// `tools/call` so a subsequent `notifications/cancelled` for the same
/// JSON-RPC `id` can wake the round-trip task and trigger a broker-side
/// cancel.
pub struct InflightEntry {
    /// Companion-minted opaque handle threaded through the broker.
    external_handle: String,
    /// Tripped by the cancel handler to wake the round-trip task.
    cancel_tx: oneshot::Sender<()>,
}

/// `request_id_key(id) → InflightEntry`. Keyed by a string form of the
/// JSON-RPC `id` so we can compare against the `requestId` payload of
/// `notifications/cancelled` which is itself a JSON value (numbers serialize
/// as their canonical string form here).
#[derive(Default)]
pub struct InflightCalls {
    inner: Mutex<HashMap<String, InflightEntry>>,
}

impl InflightCalls {
    pub fn new() -> Self {
        Self::default()
    }

    async fn register(&self, id_key: String, entry: InflightEntry) {
        self.inner.lock().await.insert(id_key, entry);
    }

    async fn take(&self, id_key: &str) -> Option<InflightEntry> {
        self.inner.lock().await.remove(id_key)
    }

    /// Drain every in-flight entry, clearing the registry. Called at
    /// companion shutdown so we can fire one broker cancel per pending
    /// delegation — without this the broker would park on `rx.await` for
    /// each entry until the parent ACP connection's `cancel_by_parent`
    /// fires (or never, if the agent CLI keeps running after only the
    /// MCP child died).
    pub async fn drain_all(&self) -> Vec<InflightEntry> {
        let mut map = self.inner.lock().await;
        map.drain().map(|(_k, v)| v).collect()
    }
}

/// Canonicalize a JSON-RPC `id` to a string suitable as a `HashMap` key.
/// JSON-RPC permits string OR number ids; we collapse both via
/// `serde_json::to_string` so a numeric `42` and string `"42"` stay
/// distinct (which the spec also requires).
pub fn request_id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| String::from("null"))
}

/// Dispatch verdict for a single inbound stdin line.
pub enum LineAction {
    /// Synchronous response — write `resp` to stdout immediately.
    Respond(JsonRpcResponse),
    /// Asynchronous tools/call — the binary should spawn the round-trip
    /// task and only write a response if the future returns `Some`.
    Spawn(SpawnedCall),
    /// Notification or no-op (parse errors with `id = null`). Nothing to
    /// emit on stdout.
    Silent,
}

/// Materialized async tools/call ready to drive in a tokio task. The binary
/// awaits `future` to obtain the optional `JsonRpcResponse` and writes
/// it out (or suppresses, on cancel).
pub struct SpawnedCall {
    /// JSON-RPC `id` of the original `tools/call` so the binary can stamp
    /// the response.
    pub request_id: Value,
    /// String form of `request_id` for inflight bookkeeping.
    pub request_id_key: String,
    /// The future that performs the UDS round-trip racing the cancel
    /// channel. `None` means cancellation won — suppress the response.
    pub future: futures_util::future::BoxFuture<'static, Option<JsonRpcResponse>>,
}

/// Parse a stdin line and produce a [`LineAction`]. The binary handles the
/// IO side; this function is pure aside from registering the inflight
/// entry on `tools/call` so unit tests can drive it without stdio.
pub async fn dispatch_line(
    ctx: &CompanionContext,
    inflight: Arc<InflightCalls>,
    line: &str,
) -> LineAction {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return LineAction::Respond(err(Value::Null, -32700, format!("parse error: {e}")));
        }
    };

    // Notifications carry no id — no response goes out. Cancellation is
    // the only notification we act on.
    if req.id.is_none() {
        if req.method == "notifications/cancelled" {
            handle_cancel_notification(ctx, &inflight, &req.params).await;
        }
        return LineAction::Silent;
    }

    let id = req.id.expect("checked is_none");
    match req.method.as_str() {
        "initialize" => LineAction::Respond(ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "codeg-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "tools": {} },
            }),
        )),
        "tools/list" => {
            let tool: Value = match serde_json::from_str(TOOL_SCHEMA_JSON) {
                Ok(v) => v,
                Err(e) => {
                    return LineAction::Respond(err(
                        id,
                        -32603,
                        format!("embedded schema invalid: {e}"),
                    ));
                }
            };
            LineAction::Respond(ok(id, json!({ "tools": [tool] })))
        }
        "tools/call" => build_tools_call_spawn(ctx.clone(), inflight, id, req.params).await,
        _ => LineAction::Respond(err(id, -32601, format!("method not found: {}", req.method))),
    }
}

/// Build the spawned-call descriptor for a `tools/call` (or, when the
/// arguments are obviously bogus, a synchronous error response). Registers
/// the inflight entry and returns a future the binary should drive.
async fn build_tools_call_spawn(
    ctx: CompanionContext,
    inflight: Arc<InflightCalls>,
    id: Value,
    params: Value,
) -> LineAction {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "delegate_to_agent" {
        return LineAction::Respond(err(id, -32602, format!("unknown tool: {name}")));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    // MCP clients (Codex / Claude Code) generally do NOT populate
    // `_meta.tool_use_id` when calling an MCP server. We still surface it
    // when present (it's the most precise binding), but a missing one is
    // expected — the broker falls back to claiming the most recent
    // `delegate_to_agent` tool_call_id observed on the parent's ACP event
    // stream.
    let tool_use_id = params
        .get("_meta")
        .and_then(|m| m.get("tool_use_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let external_handle = uuid::Uuid::new_v4().to_string();
    let req = BrokerRequest {
        token: ctx.token.clone(),
        parent_connection_id: ctx.parent_connection_id.clone(),
        parent_tool_use_id: tool_use_id,
        external_handle: Some(external_handle.clone()),
        input: arguments,
    };

    let (cancel_tx, cancel_rx) = oneshot::channel();
    let id_key = request_id_key(&id);
    inflight
        .register(
            id_key.clone(),
            InflightEntry {
                external_handle: external_handle.clone(),
                cancel_tx,
            },
        )
        .await;

    let ctx_for_task = ctx.clone();
    let id_for_response = id.clone();
    let id_key_for_task = id_key.clone();
    let inflight_for_task = inflight.clone();
    // The external_handle is only needed to write the inflight registry
    // entry above; the cancel BrokerMessage is dispatched by
    // `handle_cancel_notification`, not by the task. Keeping it bound
    // here would be dead-store noise — drop it via the `_` discard.
    let _ = external_handle;
    let future = Box::pin(async move {
        // Race the UDS round-trip against the cancel signal. Cancel wins →
        // suppress the response per MCP spec; the cancel notification
        // handler is responsible for dispatching `BrokerMessage::Cancel`
        // to the listener. Doing it from BOTH sites caused the broker's
        // `pre_canceled_handles` set to leak entries for handles that
        // were already drained by the first cancel.
        tokio::select! {
            biased;
            _ = cancel_rx => {
                // The cancel handler already pulled the inflight entry;
                // this is a defensive re-take in case some other path
                // dropped `cancel_tx` without going through the handler.
                let _ = inflight_for_task.take(&id_key_for_task).await;
                None
            }
            rt = client_round_trip(&ctx_for_task.socket_path, &req) => {
                let _ = inflight_for_task.take(&id_key_for_task).await;
                match rt {
                    Ok(resp) => Some(ok(id_for_response, render_tool_result(&resp.outcome))),
                    Err(e) => Some(err(
                        id_for_response,
                        -32603,
                        format!("broker round-trip failed: {e}"),
                    )),
                }
            }
        }
    });

    LineAction::Spawn(SpawnedCall {
        request_id: id,
        request_id_key: id_key,
        future,
    })
}

/// Handle a `notifications/cancelled` notification. Looks up the in-flight
/// call by `requestId` and fires its cancel channel. Unknown ids are
/// silently ignored per MCP spec.
async fn handle_cancel_notification(
    ctx: &CompanionContext,
    inflight: &Arc<InflightCalls>,
    params: &Value,
) {
    let request_id = match params.get("requestId") {
        Some(v) => v.clone(),
        None => return,
    };
    let id_key = request_id_key(&request_id);
    let Some(entry) = inflight.take(&id_key).await else {
        return;
    };
    let _ = entry.cancel_tx.send(());
    // Single broker-side cancel per notification: the round-trip task
    // observes `cancel_rx` and only suppresses its response. If we ALSO
    // dispatched a cancel from the task we'd hit the broker twice — the
    // first call drains the pending entry, the second buffers the handle
    // in `pre_canceled_handles` with no consumer (silent leak).
    //
    // Synchronous, bounded by `BROKER_CANCEL_BUDGET`. Detaching via
    // `tokio::spawn` would race the runtime shutdown: if stdin closes
    // before the spawned task scheduled its UDS connect, the runtime
    // drops it and the broker never gets the cancel. The bounded await
    // here guarantees the cancel either lands or hits a known cap
    // before the next stdin line is read.
    let cancel_req = BrokerCancelRequest {
        token: ctx.token.clone(),
        external_handle: entry.external_handle,
        reason: params
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };
    send_broker_cancel(&ctx.socket_path, &cancel_req).await;
}

/// Drain every in-flight `tools/call` entry and dispatch a broker cancel
/// for each. Called at companion shutdown (stdin EOF, parent-watchdog
/// fire) so the broker doesn't hold a `pending` row open forever waiting
/// for a `TurnComplete` whose response we couldn't deliver anyway. Each
/// cancel is bounded by [`BROKER_CANCEL_BUDGET`] so a hung listener
/// can't pin shutdown — the codeg main side's `cancel_by_parent` cascade
/// is the eventual backstop for any cancel that times out here.
pub async fn drain_and_cancel_all(
    ctx: &CompanionContext,
    inflight: &Arc<InflightCalls>,
    reason: &str,
) {
    for entry in inflight.drain_all().await {
        // Wake the round-trip task if it's still scheduled, so it can
        // exit promptly when the runtime tears down.
        let _ = entry.cancel_tx.send(());
        let cancel_req = BrokerCancelRequest {
            token: ctx.token.clone(),
            external_handle: entry.external_handle,
            reason: Some(reason.to_string()),
        };
        send_broker_cancel(&ctx.socket_path, &cancel_req).await;
    }
}

/// Map a serialized [`super::types::DelegationOutcome`] into MCP `tools/call`
/// result content. Kept as a separate function so unit tests can assert the
/// mapping without a real socket.
pub fn render_tool_result(outcome: &Value) -> Value {
    let kind = outcome.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let is_error = kind == "err";
    let text = if is_error {
        outcome
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("delegation failed")
            .to_string()
    } else {
        outcome
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
        "structuredContent": outcome.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CompanionContext {
        CompanionContext {
            parent_connection_id: "p1".into(),
            socket_path: "/tmp/codeg-mcp-companion-test-nope.sock".into(),
            token: "tok".into(),
        }
    }

    async fn dispatch_for_test(line: &str) -> LineAction {
        dispatch_line(&ctx(), Arc::new(InflightCalls::new()), line).await
    }

    fn unwrap_respond(action: LineAction) -> JsonRpcResponse {
        match action {
            LineAction::Respond(r) => r,
            LineAction::Spawn(_) => panic!("expected Respond, got Spawn"),
            LineAction::Silent => panic!("expected Respond, got Silent"),
        }
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "codeg-mcp");
    }

    #[tokio::test]
    async fn tools_list_returns_delegate_to_agent() {
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "delegate_to_agent");
        // Schema enumerates all 6 agent types.
        let agents = tools[0]["inputSchema"]["properties"]["agent_type"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(agents.len(), 6);
        // No more timeout_seconds property on the tool schema.
        assert!(tools[0]["inputSchema"]["properties"]
            .get("timeout_seconds")
            .is_none());
    }

    #[tokio::test]
    async fn notifications_initialized_produces_no_response() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let action = dispatch_for_test(line).await;
        assert!(matches!(action, LineAction::Silent));
    }

    #[tokio::test]
    async fn parse_error_returns_null_id_error() {
        let line = "not json";
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32700);
        assert!(e.message.contains("parse"));
        assert_eq!(resp.id, Value::Null);
    }

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32601);
    }

    #[tokio::test]
    async fn tools_call_with_unknown_tool_rejected_synchronously() {
        let line = r#"{
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params": {
                "name": "other_tool",
                "arguments": {},
                "_meta": {"tool_use_id": "tu1"}
            }
        }"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("other_tool"));
    }

    #[tokio::test]
    async fn tools_call_registers_inflight_and_returns_spawn() {
        let inflight = Arc::new(InflightCalls::new());
        let line = r#"{
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params": {
                "name": "delegate_to_agent",
                "arguments": {"agent_type": "codex", "task": "x"}
            }
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        match action {
            LineAction::Spawn(call) => {
                assert_eq!(call.request_id_key, request_id_key(&Value::from(4)));
            }
            _ => panic!("expected Spawn"),
        }
        // The inflight registry should now have an entry for id=4.
        let map = inflight.inner.lock().await;
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&request_id_key(&Value::from(4))));
    }

    #[tokio::test]
    async fn cancel_notification_fires_inflight_cancel_channel() {
        let inflight = Arc::new(InflightCalls::new());
        // Pre-seed an inflight entry with a known cancel_tx; verify the
        // notification handler trips it.
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        inflight
            .register(
                request_id_key(&Value::from(7)),
                InflightEntry {
                    external_handle: "h-7".into(),
                    cancel_tx,
                },
            )
            .await;

        let line = r#"{
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params": {"requestId": 7, "reason": "user requested"}
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        assert!(matches!(action, LineAction::Silent));
        // The cancel channel should now be tripped (best-effort
        // `client_cancel` to a bogus socket failed silently — that's fine).
        assert!(cancel_rx.try_recv().is_ok());
        // Entry has been pulled.
        let map = inflight.inner.lock().await;
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn cancel_for_unknown_request_id_is_silent_noop() {
        let inflight = Arc::new(InflightCalls::new());
        let line = r#"{
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params": {"requestId": 999}
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        assert!(matches!(action, LineAction::Silent));
        assert!(inflight.inner.lock().await.is_empty());
    }

    #[test]
    fn render_tool_result_maps_ok_outcome() {
        let outcome = json!({"kind": "ok", "text": "hi", "child_conversation_id": 42});
        let rendered = render_tool_result(&outcome);
        assert_eq!(rendered["isError"], false);
        assert_eq!(rendered["content"][0]["text"], "hi");
        assert_eq!(rendered["structuredContent"]["child_conversation_id"], 42);
    }

    #[test]
    fn render_tool_result_maps_err_outcome() {
        let outcome = json!({
            "kind": "err",
            "code": "canceled",
            "message": "canceled: user requested"
        });
        let rendered = render_tool_result(&outcome);
        assert_eq!(rendered["isError"], true);
        assert_eq!(rendered["content"][0]["text"], "canceled: user requested");
        assert_eq!(rendered["structuredContent"]["code"], "canceled");
    }
}
