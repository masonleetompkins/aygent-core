// Telegram per-agent bridge (Fix 7).
// One bot token per Agent. Token stored in macOS Keychain (never in DB/files).
// DB holds: telegram_enabled, telegram_bot_username, telegram_allowed_chats (CSV).
// Runtime: long-poll Bot API getUpdates per enabled agent; inbound messages are
// enqueued into mailbox (from_agent="telegram:<chat_id>") and run via drainer like
// any other headless turn; replies flow back via sendMessage.

use std::collections::HashSet;

pub fn keychain_service(agent_id: &str) -> String {
    format!("telegram-bot-{}", agent_id)
}

pub fn keychain_account() -> &'static str {
    "bot_token"
}

/// Parse a Telegram Bot token roughly: "123:AA...". Used to gate.
pub fn looks_like_token(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.contains(':') && s.len() > 20
}

/// Allowed chats set: empty = allow any; else only listed IDs.
pub fn allowed_set(csv: &str) -> Option<HashSet<String>> {
    let csv = csv.trim();
    if csv.is_empty() {
        return None;
    }
    let set: HashSet<String> = csv
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

pub fn chat_allowed(chat_id: &str, csv: &str) -> bool {
    match allowed_set(csv) {
        None => true,
        Some(set) => set.contains(chat_id),
    }
}

// Minimal Bot API long-poll wiring lives in this module so it doesn't inflate lib.rs.
// Spawning one tokio task per enabled agent is cheap; poll interval is adaptive
// (long-poll with timeout=25, plus 1s pause between cycles when idle).

#[derive(serde::Deserialize, Debug)]
struct UpdatesResp {
    ok: bool,
    result: Option<Vec<Update>>,
}
#[derive(serde::Deserialize, Debug)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}
#[derive(serde::Deserialize, Debug)]
struct Message {
    message_id: i64,
    chat: Chat,
    from: Option<User>,
    text: Option<String>,
}
#[derive(serde::Deserialize, Debug)]
struct Chat {
    id: i64,
}
#[derive(serde::Deserialize, Debug)]
struct User {
    id: i64,
    username: Option<String>,
}

async fn bot_get_updates(token: &str, offset: i64) -> Result<Vec<Update>, String> {
    let url = format!(
        "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout=25&allowed_updates=[\"message\"]",
        token, offset
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client.get(&url).send().await.map_err(|e| format!("getUpdates: {e}"))?;
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    let parsed: UpdatesResp = serde_json::from_str(&body).map_err(|e| format!("parse updates: {e} — body={body:.400}"))?;
    if !parsed.ok {
        return Err(format!("Telegram error: {body:.600}"));
    }
    Ok(parsed.result.unwrap_or_default())
}

async fn bot_send_message(token: &str, chat_id: &str, text: &str) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await
        .map_err(|e| format!("sendMessage: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("sendMessage failed: {body:.600}"));
    }
    Ok(())
}

async fn bot_get_me(token: &str) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{}/getMe", token);
    let body = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("getMe: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read getMe: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(format!("getMe failed: {body:.400}"));
    }
    Ok(v
        .get("result")
        .and_then(|r| r.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string())
}

/// Spawn one polling worker per enabled agent (called on boot + after edits).
/// Each worker long-polls its bot's getUpdates; each inbound message is enqueued
/// into the agent's mailbox so the existing drainer runs it headlessly.
pub fn spawn_all(app: tauri::AppHandle, db: crate::writer::Db) {
    // Enumerate enabled agents snapshot; spawn one task per agent.
    let agents = match crate::repo::list_agents(&db) {
        Ok(a) => a.into_iter().filter(|x| x.telegram_enabled).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for a in agents {
        let app2 = app.clone();
        let db2 = db.clone();
        tauri::async_runtime::spawn(async move {
            run_agent_loop(app2, db2, a.id).await;
        });
    }
}

async fn run_agent_loop(app: tauri::AppHandle, db: crate::writer::Db, agent_id: String) {
    // Resolve token from keychain; if missing, worker just exits (re-spawn on next edit).
    let token = match crate::keychain::get_key(&keychain_service(&agent_id)) {
        Ok(t) if looks_like_token(&t) => t,
        _ => {
            eprintln!("[telegram] agent {} has no token — worker not starting", agent_id);
            return;
        }
    };
    let mut offset: i64 = 0;
    // On first boot, skip old backlog so we don't replay weeks of history.
    // getUpdates without offset returns recent updates; we consume them silently once.
    if let Ok(ups) = bot_get_updates(&token, 0).await {
        if let Some(max) = ups.iter().map(|u| u.update_id).max() {
            offset = max + 1;
        }
    }
    eprintln!("[telegram] polling for agent {}", agent_id);
    loop {
        // Check agent still enabled (edits can disable it).
        let still = crate::repo::get_agent(&db, &agent_id)
            .ok()
            .flatten()
            .map(|ag| ag.telegram_enabled)
            .unwrap_or(false);
        if !still {
            eprintln!("[telegram] agent {} disabled — worker exiting", agent_id);
            break;
        }
        match bot_get_updates(&token, offset).await {
            Ok(updates) => {
                for u in updates {
                    offset = offset.max(u.update_id + 1);
                    let Some(msg) = u.message else { continue };
                    let chat_id = msg.chat.id.to_string();
                    let text = msg.text.unwrap_or_default().trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    // Allowlist check (fresh read so edits apply immediately).
                    let allowed_csv = crate::repo::get_agent(&db, &agent_id)
                        .ok()
                        .flatten()
                        .map(|ag| ag.telegram_allowed_chats)
                        .unwrap_or_default();
                    if !chat_allowed(&chat_id, &allowed_csv) {
                        eprintln!("[telegram] blocked chat {} for agent {} (not in allowlist)", chat_id, agent_id);
                        continue;
                    }
                    // Enqueue into mailbox: from = telegram:<chat_id> so reply can route back.
                    let from = format!("telegram:{chat_id}");
                    let body = text.clone();
                    let dbc = db.clone();
                    let aid = agent_id.clone();
                    let ok = dbc
                        .write(move |c| {
                            c.execute(
                                "INSERT INTO mailbox (from_agent,to_agent,body,root_id,depth,ancestry,status,created_at) VALUES (?1,?2,?3,0,6,'','pending',?4)",
                                rusqlite::params![from, aid, body, crate::repo::now_ms()],
                            )
                            .map_err(|e| format!("enqueue telegram: {e}"))?;
                            let mid = c.last_insert_rowid();
                            c.execute("UPDATE mailbox SET root_id = ?1 WHERE id = ?1", rusqlite::params![mid])
                                .map_err(|e| format!("set root: {e}"))?;
                            c.execute(
                                "INSERT OR IGNORE INTO mailbox_budget (root_id, turns, cap, created_at) VALUES (?1, 0, 12, ?2)",
                                rusqlite::params![mid, crate::repo::now_ms()],
                            )
                            .map_err(|e| format!("budget: {e}"))?;
                            Ok::<_, String>(())
                        })
                        .is_ok();
                    if ok {
                        // Wake drainer
                        use tauri::Manager;
                        if let Some(sig) = app.try_state::<crate::drainer::DrainSignal>() {
                            sig.inner().clone().nudge();
                        }
                        // Also remember last chat id so replies have a target when no allowlist
                        let _ = crate::keychain::set_key(&format!("telegram-last-chat-{}", agent_id), &chat_id);
                    }
                }
            }
            Err(e) => {
                eprintln!("[telegram] poll error agent {}: {e}", agent_id);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

/// Send a reply back to the originating Telegram chat. Called after a headless
/// turn that was triggered by telegram:<chat_id> completes.
pub async fn reply_to_origin(agent_id: &str, origin: &str, text: &str) {
    let Some(chat_id) = origin.strip_prefix("telegram:") else { return };
    let token = match crate::keychain::get_key(&keychain_service(agent_id)) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return,
    };
    let clipped = if text.chars().count() > 3500 {
        let t: String = text.chars().take(3500).collect();
        format!("{t}\n…(truncated)")
    } else {
        text.to_string()
    };
    if let Err(e) = bot_send_message(&token, chat_id, &clipped).await {
        eprintln!("[telegram] reply failed agent {} chat {}: {e}", agent_id, chat_id);
    }
}

/// Validate a token by calling getMe; returns the bot username on success.
pub async fn validate_token(token: &str) -> Result<String, String> {
    let t = token.trim();
    if !looks_like_token(t) {
        return Err("that doesn't look like a Telegram Bot token (expected 123456:AA…)".into());
    }
    bot_get_me(t).await
}
