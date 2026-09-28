// V2 CRYPTO PROOF: XChaCha20-Poly1305 IETF AEAD with a fixed 32-byte key.
// Rust seals, JS (libsodium crypto_aead_xchacha20poly1305_ietf) opens; then
// JS seals a reply and Rust opens it via `open` mode.
// Run: cargo run --example crypto_interop_v2            (prints envelope json)
//      cargo run --example crypto_interop_v2 -- open <nonce_b64> <ct_b64>
use aygent_lib::remote::Sealer;
use base64::Engine;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

fn main() {
    let sealer = Sealer::from_key([9u8; 32]); // fixed TEST key
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 4 && args[1] == "open" {
        let env = aygent_lib::remote::Envelope {
            v: 2, from: "web".into(), turn: "interop".into(),
            seq: 0, last: true,
            nonce: args[2].clone(), ct: args[3].clone(),
        };
        match sealer.open(&env) {
            Ok(p) => println!("RUST_OPENED: {}", String::from_utf8_lossy(&p)),
            Err(e) => { println!("RUST_OPEN_FAILED: {e}"); std::process::exit(1); }
        }
        return;
    }

    let envs = sealer.seal("interop", b"interop-v2:rust->js").expect("seal");
    println!(
        "{}",
        serde_json::json!({
            "key": B64.encode([9u8; 32]),
            "nonce": envs[0].nonce,
            "ct": envs[0].ct,
        })
    );
}
