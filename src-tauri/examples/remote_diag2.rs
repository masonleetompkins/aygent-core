// RAW JOIN DIAGNOSTIC: connect to Realtime directly and PRINT the full
// phx_reply payload for the private-channel join — the refusal reason my
// remote_rt client discards. Run: cargo run --example remote_diag2
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let meta = aygent_lib::remote::load_meta().expect("paired");
    let jwt = aygent_lib::remote::load_jwt().expect("jwt");

    // Same coords the runtime uses (from the site).
    let client = reqwest::Client::new();
    let v: serde_json::Value = client
        .get(format!("{}/api/remote/device", meta.site))
        .bearer_auth(&jwt)
        .send().await.unwrap().json().await.unwrap();
    let supa_url = v["supabase_url"].as_str().unwrap().to_string();
    let anon = v["supabase_anon"].as_str().unwrap().to_string();

    let ws_url = format!(
        "{}/realtime/v1/websocket?apikey={}&vsn=1.0.0",
        supa_url.replace("https://", "wss://"),
        anon
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.expect("ws connect");
    let topic = format!("realtime:{}", meta.channel);

    let join = serde_json::json!({
        "topic": topic,
        "event": "phx_join",
        "payload": {
            "config": { "broadcast": { "self": false }, "private": true },
            "access_token": jwt,
        },
        "ref": "1",
    });
    ws.send(Message::Text(join.to_string())).await.unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        tokio::select! {
            frame = ws.next() => match frame {
                Some(Ok(Message::Text(t))) => {
                    println!("FRAME: {t}");
                    if t.contains("phx_reply") { break; }
                }
                Some(Ok(_)) => {}
                other => { println!("SOCKET: {other:?}"); break; }
            },
            _ = tokio::time::sleep_until(deadline) => { println!("TIMEOUT"); break; }
        }
    }
}
