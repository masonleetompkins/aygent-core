// LIVE DIAGNOSTIC: walk the exact chain remote_runtime::start_if_paired walks,
// with the REAL keychain credentials, printing where it breaks.
//   1. keychain meta + jwt present?
//   2. GET {site}/api/remote/device with the device JWT — status? browser key?
//      supabase coords?
//   3. join the real private channel with the device JWT — Connected or not?
// Run: cargo run --example remote_diag
use aygent_lib::remote_rt::{spawn, RtEvent};

#[tokio::main]
async fn main() {
    // 1) keychain
    let Some(meta) = aygent_lib::remote::load_meta() else {
        println!("STEP1 FAIL: no remote meta in keychain (not paired?)");
        return;
    };
    println!("STEP1 OK: paired. site={} channel={} device_id={}", meta.site, meta.channel, meta.device_id);
    let Some(jwt) = aygent_lib::remote::load_jwt() else {
        println!("STEP1 FAIL: meta present but no device JWT");
        return;
    };
    println!("STEP1 OK: jwt present ({} chars)", jwt.len());

    // 2) site API
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap();
    let resp = client
        .get(format!("{}/api/remote/device", meta.site))
        .bearer_auth(&jwt)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => { println!("STEP2 FAIL: request error: {e}"); return; }
    };
    let status = resp.status();
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => { println!("STEP2 FAIL: HTTP {status}, body not json: {e}"); return; }
    };
    println!("STEP2: HTTP {status}");
    let dev = v.get("device").cloned().unwrap_or_default();
    let browser_pub = dev.get("browser_pubkey").and_then(|k| k.as_str()).unwrap_or("");
    let sas_ok = dev.get("sas_confirmed").and_then(|b| b.as_bool()).unwrap_or(false);
    let supa_url = v.get("supabase_url").and_then(|u| u.as_str()).unwrap_or("");
    let supa_anon = v.get("supabase_anon").and_then(|k| k.as_str()).unwrap_or("");
    println!(
        "STEP2: browser_pubkey={} sas_confirmed={} supabase_url={} anon={}",
        if browser_pub.is_empty() { "MISSING" } else { "present" },
        sas_ok,
        if supa_url.is_empty() { "MISSING" } else { supa_url },
        if supa_anon.is_empty() { "MISSING" } else { "present" },
    );
    if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        println!("STEP2 FAIL: body={v}");
        return;
    }
    if browser_pub.is_empty() || supa_url.is_empty() || supa_anon.is_empty() {
        println!("STEP2 FAIL: start_if_paired would bail here");
        return;
    }

    // 3) real private-channel join
    println!("STEP3: joining {} as device...", meta.channel);
    let (_client, mut events) = spawn(
        supa_url.to_string(),
        supa_anon.to_string(),
        meta.channel.clone(),
        jwt,
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        tokio::select! {
            evt = events.recv() => match evt {
                Some(RtEvent::Connected) => { println!("STEP3 OK: JOINED private channel — the full chain works"); return; }
                Some(RtEvent::Disconnected { retry_in_secs }) => {
                    println!("STEP3 FAIL: join refused/disconnected (retry_in={retry_in_secs}s) — RLS/JWT/private-channel problem");
                    return;
                }
                Some(RtEvent::Inbound(_)) => {}
                None => { println!("STEP3 FAIL: event stream closed"); return; }
            },
            _ = tokio::time::sleep_until(deadline) => { println!("STEP3 FAIL: 20s timeout with no join result"); return; }
        }
    }
}
