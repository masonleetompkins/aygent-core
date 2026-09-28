// LIVE HARNESS: prove the Rust Realtime client (remote_rt) against the REAL
// Supabase project, talking to the same JS client the browser will use.
//
// Run:  cargo run --example rt_harness
// Pairs with remote-spike/harness_web.js (the "browser" side).
//
// Uses a PUBLIC channel (no JWT) because the RLS policy + device JWT don't
// exist until Mason applies migration 004. This proves FRAMING + TRANSPORT;
// auth is proven after the migration. Sends "env" envelopes with dummy
// ciphertext — the crypto is already unit-proven, this is about the wire.

use aygent_lib::remote::Envelope;
use aygent_lib::remote_rt::{spawn, RtCommand, RtEvent};

#[tokio::main]
async fn main() {
    let url = "https://uxlhgulwwgwbtvpsqgtu.supabase.co".to_string();
    let anon = "sb_publishable_122OmSKtREH0uAcMMOXPDA_LvYOEOxr".to_string();
    let channel = std::env::args().nth(1).unwrap_or("remote:rust-harness".to_string());

    let (client, mut events) = spawn(url, anon, channel.clone(), String::new());
    println!("[rust] connecting to channel {channel}");

    let mut got_inbound = 0u32;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

    loop {
        tokio::select! {
            evt = events.recv() => match evt {
                Some(RtEvent::Connected) => {
                    println!("[rust] CONNECTED — sending hello env");
                    let env = Envelope {
                        v: 1, from: "dev".into(), turn: "harness".into(),
                        seq: 0, last: true,
                        nonce: "bm9uY2U=".into(), ct: "aGVsbG8=".into(),
                    };
                    let _ = client.tx.send(RtCommand::Send(env)).await;
                }
                Some(RtEvent::Inbound(env)) => {
                    got_inbound += 1;
                    println!("[rust] INBOUND from={} turn={} seq={} last={}",
                             env.from, env.turn, env.seq, env.last);
                    // Echo back so the JS side can verify rust→js too.
                    let echo = Envelope {
                        v: 1, from: "dev".into(), turn: env.turn.clone(),
                        seq: env.seq, last: env.last,
                        nonce: "bm9uY2U=".into(), ct: "ZWNobw==".into(),
                    };
                    let _ = client.tx.send(RtCommand::Send(echo)).await;
                    if got_inbound >= 3 {
                        println!("[rust] SUCCESS: 3 inbound envelopes received + echoed");
                        let _ = client.tx.send(RtCommand::Shutdown).await;
                        return;
                    }
                }
                Some(RtEvent::Disconnected { retry_in_secs }) => {
                    println!("[rust] disconnected, retry in {retry_in_secs}s");
                }
                None => { println!("[rust] event stream closed"); return; }
            },
            _ = tokio::time::sleep_until(deadline) => {
                println!("[rust] TIMEOUT: got {got_inbound} inbound (wanted 3)");
                std::process::exit(1);
            }
        }
    }
}
