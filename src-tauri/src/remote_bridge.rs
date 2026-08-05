// AYGENT REMOTE — the turn bridge (R4).
//
// Sits between the Realtime channel (sealed envelopes) and the agent engine.
// This module owns the PROTOCOL: decrypted message shapes, dispatch, delta
// coalescing, and turn dedupe. It deliberately does NOT own turn execution —
// lib.rs hands it callbacks wired to the real machinery (db/broker/lanes), so
// everything here is unit-testable without an AppHandle.
//
// Decrypted message types (see context/plan-aygent-remote.md §3):
//   web→dev: hello, list_agents, list_convs{agent}, open_conv{id},
//            new_conv{agent}, prompt{turn, agent, conv, text}, stop{turn}, ping
//   dev→web: hello{device_name, agents[]}, convs{agent, convs[]},
//            conv_history{id, msgs}, turn_start{turn}, delta{turn, text},
//            tool_start{turn, name, summary}, tool_end{turn, name, ok},
//            turn_end{turn}, error{msg}, pong
//
// SECURITY: capability is decided HERE on the device, never by the server.
// v1 remote turns run with the agent's normal tool inventory MINUS Pro-Mode
// shell (exec) unless the user flips "remote writes" in Settings — the same
// one-gate discipline as connections (the off-list is the single gate; this
// adds one device-local remote gate in front of exec only).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// PROTOCOL TYPES
// ---------------------------------------------------------------------------

/// Everything the browser can ask of the device.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WebMsg {
    Hello,
    ListAgents,
    ListConvs { agent: String },
    OpenConv { id: String },
    NewConv { agent: String },
    Prompt { turn: String, agent: String, conv: String, text: String },
    Stop { turn: String },
    Ping,
}

/// Everything the device says back. Serialized, sealed, chunked, broadcast.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DevMsg {
    Hello { device_name: String, agents: Vec<AgentInfo> },
    Convs { agent: String, convs: serde_json::Value },
    ConvHistory { id: String, title: String, msgs: serde_json::Value },
    TurnStart { turn: String },
    Delta { turn: String, text: String },
    ToolStart { turn: String, name: String, summary: String },
    ToolEnd { turn: String, name: String, ok: bool },
    TurnEnd { turn: String },
    Error { msg: String },
    Pong,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
}

/// Parse a decrypted plaintext into a WebMsg. Unknown/garbage → None (the
/// bridge ignores rather than errors: a newer web client may send types this
/// build doesn't know, and that must not kill the session).
pub fn parse_web_msg(plain: &[u8]) -> Option<WebMsg> {
    serde_json::from_slice(plain).ok()
}

// ---------------------------------------------------------------------------
// DELTA COALESCING — ≤10 msgs/sec toward Realtime (rate limits + quota).
// Buffers TextDeltas; flushes on interval tick, on any non-delta event, or
// when the buffer crosses one chunk (48KB) so a single flush never multi-chunks.
// ---------------------------------------------------------------------------

pub struct Coalescer {
    turn: String,
    buf: String,
    last_flush: Instant,
    min_gap: Duration,
}

impl Coalescer {
    pub fn new(turn: &str) -> Self {
        Self {
            turn: turn.to_string(),
            buf: String::new(),
            last_flush: Instant::now() - Duration::from_secs(1),
            min_gap: Duration::from_millis(100), // 10/sec
        }
    }

    /// Add streamed text. Returns a Delta to send NOW if the gap has passed
    /// or the buffer is large; otherwise buffers (call `flush` on tick/end).
    pub fn push(&mut self, text: &str) -> Option<DevMsg> {
        self.buf.push_str(text);
        let big = self.buf.len() >= crate::remote::CHUNK_BYTES / 2;
        if big || self.last_flush.elapsed() >= self.min_gap {
            return self.flush();
        }
        None
    }

    /// Emit whatever is buffered (turn end, non-delta event, or timer tick).
    pub fn flush(&mut self) -> Option<DevMsg> {
        if self.buf.is_empty() {
            return None;
        }
        self.last_flush = Instant::now();
        Some(DevMsg::Delta { turn: self.turn.clone(), text: std::mem::take(&mut self.buf) })
    }
}

// ---------------------------------------------------------------------------
// TURN DEDUPE — Realtime is at-least-once under reconnects; the browser also
// retries prompts it isn't sure landed. The turn uuid is generated web-side;
// a duplicate prompt with a seen id is dropped here, before any engine work.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TurnDedupe {
    seen: HashSet<String>,
    order: Vec<String>, // insertion order for cheap eviction
}

impl TurnDedupe {
    const CAP: usize = 256;

    /// True the FIRST time a turn id is offered; false for replays.
    pub fn accept(&mut self, turn: &str) -> bool {
        if self.seen.contains(turn) {
            return false;
        }
        self.seen.insert(turn.to_string());
        self.order.push(turn.to_string());
        if self.order.len() > Self::CAP {
            let old = self.order.remove(0);
            self.seen.remove(&old);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// STREAM-EVENT TRANSLATION — the existing engine already emits StreamEvents
// on the conversation channel; the bridge listens and translates. This is the
// 1:1 mapping the spike promised: no new engine, just a different renderer.
// ---------------------------------------------------------------------------

/// Translate one emitted StreamEvent JSON payload into 0..1 DevMsg (plus
/// coalescing for text). `ev` is the JSON the engine emits via app.emit.
pub fn translate_event(turn: &str, ev: &serde_json::Value, co: &mut Coalescer) -> Vec<DevMsg> {
    let mut out = Vec::new();
    // Engine events are externally-tagged enums: {"TextDelta":{"text":..}} etc.
    // (turns.ts does this same sniff on the UI side.)
    if let Some(d) = ev.get("TextDelta") {
        if let Some(t) = d.get("text").and_then(|t| t.as_str()) {
            if let Some(m) = co.push(t) {
                out.push(m);
            }
        }
        return out;
    }
    // Any non-delta event flushes buffered text first so ordering is preserved.
    if let Some(m) = co.flush() {
        out.push(m);
    }
    if let Some(d) = ev.get("ToolUse") {
        let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("tool").to_string();
        out.push(DevMsg::ToolStart { turn: turn.into(), name, summary: String::new() });
    } else if let Some(d) = ev.get("ToolResult") {
        let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("tool").to_string();
        let ok = d.get("ok").and_then(|o| o.as_bool()).unwrap_or(true);
        out.push(DevMsg::ToolEnd { turn: turn.into(), name, ok });
    } else if ev.get("Done").is_some() {
        out.push(DevMsg::TurnEnd { turn: turn.into() });
    } else if let Some(d) = ev.get("Error") {
        let msg = d.get("text").and_then(|t| t.as_str()).unwrap_or("turn failed").to_string();
        out.push(DevMsg::Error { msg });
    }
    // Info / ToolUseStart / ToolUseDelta are intentionally not forwarded in v1:
    // args can contain file contents (bandwidth) and Info is local color.
    out
}

// ---------------------------------------------------------------------------
// SEND HELPER — serialize a DevMsg, seal, and hand envelopes to the rt client.
// ---------------------------------------------------------------------------

pub async fn send_dev_msg(
    sealer: &crate::remote::Sealer,
    tx: &tokio::sync::mpsc::Sender<crate::remote_rt::RtCommand>,
    turn_scope: &str,
    msg: &DevMsg,
) -> Result<(), String> {
    let plain = serde_json::to_vec(msg).map_err(|e| format!("encode: {e}"))?;
    // Envelope turn id: protocol replies use a scope id ("ctl" for control
    // replies, the turn uuid for turn streams) so the web side reassembles
    // interleaved streams independently.
    for env in sealer.seal(turn_scope, &plain)? {
        tx.send(crate::remote_rt::RtCommand::Send(env))
            .await
            .map_err(|_| "rt task gone".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_web_messages() {
        let cases: Vec<(&str, WebMsg)> = vec![
            (r#"{"t":"hello"}"#, WebMsg::Hello),
            (r#"{"t":"list_agents"}"#, WebMsg::ListAgents),
            (r#"{"t":"list_convs","agent":"a1"}"#, WebMsg::ListConvs { agent: "a1".into() }),
            (r#"{"t":"open_conv","id":"c1"}"#, WebMsg::OpenConv { id: "c1".into() }),
            (r#"{"t":"new_conv","agent":"a1"}"#, WebMsg::NewConv { agent: "a1".into() }),
            (
                r#"{"t":"prompt","turn":"u1","agent":"a1","conv":"c1","text":"hi"}"#,
                WebMsg::Prompt { turn: "u1".into(), agent: "a1".into(), conv: "c1".into(), text: "hi".into() },
            ),
            (r#"{"t":"stop","turn":"u1"}"#, WebMsg::Stop { turn: "u1".into() }),
            (r#"{"t":"ping"}"#, WebMsg::Ping),
        ];
        for (json, want) in cases {
            assert_eq!(parse_web_msg(json.as_bytes()).expect(json), want);
        }
    }

    #[test]
    fn unknown_or_garbage_messages_are_ignored_not_fatal() {
        assert!(parse_web_msg(br#"{"t":"future_feature","x":1}"#).is_none());
        assert!(parse_web_msg(b"not json").is_none());
        assert!(parse_web_msg(br#"{"no_tag":true}"#).is_none());
    }

    #[test]
    fn coalescer_buffers_within_gap_and_flushes_in_order() {
        let mut co = Coalescer::new("t1");
        // First push: last_flush is in the past → immediate delta.
        let first = co.push("Hel");
        assert!(matches!(first, Some(DevMsg::Delta { ref text, .. }) if text == "Hel"));
        // Within the 100ms gap: buffered.
        assert!(co.push("lo ").is_none());
        assert!(co.push("world").is_none());
        // Explicit flush emits the concatenation.
        match co.flush() {
            Some(DevMsg::Delta { text, .. }) => assert_eq!(text, "lo world"),
            other => panic!("expected buffered delta, got {other:?}"),
        }
        // Nothing left.
        assert!(co.flush().is_none());
    }

    #[test]
    fn dedupe_accepts_once_and_evicts_oldest() {
        let mut d = TurnDedupe::default();
        assert!(d.accept("a"));
        assert!(!d.accept("a"), "replay must be dropped");
        for i in 0..TurnDedupe::CAP {
            assert!(d.accept(&format!("t{i}")));
        }
        // "a" was evicted (cap exceeded) → accepted again; recent ids still deduped.
        assert!(d.accept("a"));
        assert!(!d.accept(&format!("t{}", TurnDedupe::CAP - 1)));
    }

    #[test]
    fn translates_engine_events_and_preserves_text_ordering() {
        let mut co = Coalescer::new("t1");
        // Prime the coalescer so subsequent pushes buffer.
        let _ = co.push("first ");
        let buffered = translate_event("t1", &serde_json::json!({"TextDelta": {"text": "reply"}}), &mut co);
        assert!(buffered.is_empty(), "within gap → buffered");
        // A tool event must flush the buffered text BEFORE the tool msg.
        let msgs = translate_event(
            "t1",
            &serde_json::json!({"ToolUse": {"id": "x", "name": "read_file", "input": {}}}),
            &mut co,
        );
        assert_eq!(msgs.len(), 2);
        assert!(matches!(&msgs[0], DevMsg::Delta { text, .. } if text == "reply"));
        assert!(matches!(&msgs[1], DevMsg::ToolStart { name, .. } if name == "read_file"));
        // Done → TurnEnd.
        let done = translate_event("t1", &serde_json::json!({"Done": {"stop_reason": "end_turn"}}), &mut co);
        assert!(matches!(&done[0], DevMsg::TurnEnd { .. }));
    }
}
