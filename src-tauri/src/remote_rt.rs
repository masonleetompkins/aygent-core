// AYGENT REMOTE — Supabase Realtime client (Phoenix channels over WSS).
//
// The Mac end of the relay. Connects OUT to Supabase Realtime, joins the
// private channel `remote:{user_id}` with the device JWT (RLS authorizes the
// join because the JWT's sub == the channel owner), heartbeats every 25s, and
// shuttles sealed envelopes between the channel and the turn bridge.
//
// Protocol notes (verified against the JS client's traffic during the spike):
//   connect  wss://<proj>.supabase.co/realtime/v1/websocket?apikey=<anon>&vsn=1.0.0
//   join     {topic:"realtime:remote:{uid}", event:"phx_join",
//             payload:{config:{broadcast:{self:false}, private:<bool>},
//                      access_token:<device JWT, or the anon key on a public
//                      channel — this mirrors what supabase-js sends>}, ref:"1"}
//   reply    {event:"phx_reply", ref:<join ref>, payload:{status:"ok"|"error"}}
//            — the join is NOT complete until this arrives. Emitting Connected
//            before the reply was the harness bug: a refused join (private:true
//            with an empty token) looked "connected" while broadcasts silently
//            went nowhere.
//   heartbeat every 25s: {topic:"phoenix", event:"heartbeat", payload:{}, ref:n}
//   broadcast out: {topic, event:"broadcast",
//                   payload:{type:"broadcast", event:"env", payload:<Envelope>}, ref:n}
//   broadcast in:  event=="broadcast", payload.payload==<Envelope>
//
// RECONNECT: exponential backoff 1s→60s with jitter. The channel is rejoined
// from scratch each time (Realtime holds no per-client state we care about).

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::remote::Envelope;

/// Commands the app can push to the connection task.
pub enum RtCommand {
    /// Send a sealed envelope to the channel.
    Send(Envelope),
    /// Tear down (unpair / app quit).
    Shutdown,
}

/// Events the connection task surfaces to the app.
#[derive(Debug)]
pub enum RtEvent {
    Connected,
    Disconnected { retry_in_secs: u64 },
    /// A sealed envelope arrived from the browser side.
    Inbound(Envelope),
}

#[derive(Serialize)]
struct PhxMsg<'a, P: Serialize> {
    topic: &'a str,
    event: &'a str,
    payload: P,
    #[serde(rename = "ref")]
    msg_ref: String,
}

pub struct RealtimeClient {
    pub tx: mpsc::Sender<RtCommand>,
}

/// Spawn the connection task. `supabase_url` like https://xyz.supabase.co,
/// `anon_key` the public key, `channel` like remote:{user_id}, `jwt` the device
/// token (empty = join as a PUBLIC channel with the anon key — harness/dev
/// mode until migration 004 lands). Events stream out on the returned receiver.
pub fn spawn(
    supabase_url: String,
    anon_key: String,
    channel: String,
    jwt: String,
) -> (RealtimeClient, mpsc::Receiver<RtEvent>) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<RtCommand>(64);
    let (evt_tx, evt_rx) = mpsc::channel::<RtEvent>(256);
    let shutdown = Arc::new(AtomicBool::new(false));

    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let mut backoff_secs = 1u64;
            let msg_ref = AtomicU64::new(1);
            let ws_url = format!(
                "{}/realtime/v1/websocket?apikey={}&vsn=1.0.0",
                supabase_url.replace("https://", "wss://").trim_end_matches('/'),
                anon_key
            );
            let topic = format!("realtime:{channel}");
            // Private channel iff we hold a device JWT; otherwise mirror
            // supabase-js on a public channel (access_token = the anon key).
            let is_private = !jwt.is_empty();
            let access_token = if is_private { jwt.clone() } else { anon_key.clone() };

            'reconnect: loop {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let conn = tokio_tungstenite::connect_async(&ws_url).await;
                let (mut ws, _) = match conn {
                    Ok(ok) => ok,
                    Err(_) => {
                        let _ = evt_tx.send(RtEvent::Disconnected { retry_in_secs: backoff_secs }).await;
                        tokio::time::sleep(std::time::Duration::from_secs(jittered(backoff_secs))).await;
                        backoff_secs = (backoff_secs * 2).min(60);
                        continue;
                    }
                };

                // Join the channel. `ref` is tracked so we can match the reply.
                let join_ref = next_ref(&msg_ref);
                let join = PhxMsg {
                    topic: &topic,
                    event: "phx_join",
                    payload: serde_json::json!({
                        "config": { "broadcast": { "self": false }, "private": is_private },
                        "access_token": access_token,
                    }),
                    msg_ref: join_ref.clone(),
                };
                if ws.send(Message::Text(serde_json::to_string(&join).unwrap())).await.is_err() {
                    continue 'reconnect;
                }

                // Connected ONLY once the server confirms the join. A refused
                // join (bad JWT, RLS deny) must surface as Disconnected+retry,
                // not a false Connected with a dead channel.
                if !wait_join_ok(&mut ws, &join_ref).await {
                    let _ = evt_tx.send(RtEvent::Disconnected { retry_in_secs: backoff_secs }).await;
                    tokio::time::sleep(std::time::Duration::from_secs(jittered(backoff_secs))).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue 'reconnect;
                }
                let _ = evt_tx.send(RtEvent::Connected).await;
                backoff_secs = 1; // reset after a successful join

                let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(25));
                heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                loop {
                    tokio::select! {
                        // Outbound commands from the app.
                        cmd = cmd_rx.recv() => match cmd {
                            Some(RtCommand::Send(env)) => {
                                let m = PhxMsg {
                                    topic: &topic,
                                    event: "broadcast",
                                    payload: serde_json::json!({
                                        "type": "broadcast", "event": "env", "payload": env,
                                    }),
                                    msg_ref: next_ref(&msg_ref),
                                };
                                if ws.send(Message::Text(serde_json::to_string(&m).unwrap())).await.is_err() {
                                    continue 'reconnect;
                                }
                            }
                            Some(RtCommand::Shutdown) | None => {
                                shutdown.store(true, Ordering::Relaxed);
                                let _ = ws.close(None).await;
                                break 'reconnect;
                            }
                        },
                        // Heartbeat keeps the socket alive (Phoenix drops idle conns).
                        _ = heartbeat.tick() => {
                            let hb = PhxMsg {
                                topic: "phoenix", event: "heartbeat",
                                payload: serde_json::json!({}),
                                msg_ref: next_ref(&msg_ref),
                            };
                            if ws.send(Message::Text(serde_json::to_string(&hb).unwrap())).await.is_err() {
                                continue 'reconnect;
                            }
                        }
                        // Inbound frames.
                        frame = ws.next() => match frame {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(env) = parse_broadcast(&text) {
                                    let _ = evt_tx.send(RtEvent::Inbound(env)).await;
                                }
                            }
                            Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => continue 'reconnect,
                            _ => {}
                        },
                    }
                }
            }
        }
    });

    (RealtimeClient { tx: cmd_tx }, evt_rx)
}

/// Read frames until the join reply with `join_ref` arrives (or 10s timeout /
/// socket death). Returns true only on `phx_reply` with `status: "ok"`.
async fn wait_join_ok<S>(ws: &mut S, join_ref: &str) -> bool
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let frame = tokio::select! {
            f = ws.next() => f,
            _ = tokio::time::sleep_until(deadline) => return false,
        };
        let text = match frame {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(_)) => continue,
            _ => return false,
        };
        if let Some(status) = parse_join_reply(&text, join_ref) {
            return status == "ok";
        }
    }
}

/// If `text` is the phx_reply for `join_ref`, return its status string.
fn parse_join_reply(text: &str, join_ref: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("event")?.as_str()? != "phx_reply" || v.get("ref")?.as_str()? != join_ref {
        return None;
    }
    Some(v.get("payload")?.get("status")?.as_str()?.to_string())
}

fn next_ref(counter: &AtomicU64) -> String {
    counter.fetch_add(1, Ordering::Relaxed).to_string()
}

/// 0.5x–1.5x jitter so a fleet of reconnecting devices doesn't thundering-herd.
fn jittered(secs: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    (secs / 2 + nanos % secs.max(1)).max(1)
}

/// Extract a sealed Envelope from an inbound Phoenix broadcast frame.
/// Everything else (join replies, presence, system) returns None.
fn parse_broadcast(text: &str) -> Option<Envelope> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("event")?.as_str()? != "broadcast" {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("event")?.as_str()? != "env" {
        return None;
    }
    serde_json::from_value(payload.get("payload")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inbound parser must accept exactly the frame shape Supabase sends
    /// (captured during the spike) and reject everything else on the socket.
    #[test]
    fn parses_the_real_broadcast_frame_shape() {
        let frame = r#"{
            "topic":"realtime:remote:u1","event":"broadcast",
            "payload":{"type":"broadcast","event":"env","payload":{
                "v":1,"from":"web","turn":"t1","seq":0,"last":true,
                "nonce":"bm9uY2U=","ct":"Y3Q="
            }},"ref":null}"#;
        let env = parse_broadcast(frame).expect("must parse");
        assert_eq!(env.from, "web");
        assert_eq!(env.turn, "t1");
        assert!(env.last);
    }

    #[test]
    fn ignores_join_replies_heartbeats_and_other_events() {
        for frame in [
            r#"{"topic":"realtime:remote:u1","event":"phx_reply","payload":{"status":"ok"},"ref":"1"}"#,
            r#"{"topic":"phoenix","event":"phx_reply","payload":{},"ref":"2"}"#,
            r#"{"topic":"realtime:remote:u1","event":"presence_state","payload":{},"ref":null}"#,
            r#"{"topic":"realtime:remote:u1","event":"broadcast","payload":{"type":"broadcast","event":"other","payload":{}},"ref":null}"#,
            "not json at all",
        ] {
            assert!(parse_broadcast(frame).is_none(), "should ignore: {frame}");
        }
    }

    /// Join replies: only the matching ref counts, and status is passed through
    /// verbatim ("ok" vs "error" — the caller decides connected vs retry).
    #[test]
    fn join_reply_matches_ref_and_reports_status() {
        let ok = r#"{"topic":"realtime:remote:u1","event":"phx_reply","payload":{"status":"ok","response":{}},"ref":"1"}"#;
        let err = r#"{"topic":"realtime:remote:u1","event":"phx_reply","payload":{"status":"error","response":{"reason":"unauthorized"}},"ref":"1"}"#;
        assert_eq!(parse_join_reply(ok, "1").as_deref(), Some("ok"));
        assert_eq!(parse_join_reply(err, "1").as_deref(), Some("error"));
        assert!(parse_join_reply(ok, "2").is_none(), "wrong ref must not match");
        let bcast = r#"{"event":"broadcast","payload":{},"ref":"1"}"#;
        assert!(parse_join_reply(bcast, "1").is_none());
    }

    #[test]
    fn jitter_stays_in_bounds_and_never_zero() {
        for s in [1u64, 2, 8, 60] {
            for _ in 0..50 {
                let j = jittered(s);
                assert!(j >= 1 && j <= s / 2 + s, "jitter {j} out of bounds for {s}");
            }
        }
    }
}
