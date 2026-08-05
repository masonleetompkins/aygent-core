// CROSS-LANGUAGE CRYPTO PROOF: seal envelopes with Rust's ChaChaBox using
// fixed test keys, print JSON. remote-spike/crypto_interop.js opens them with
// libsodium-wrappers-sumo (the browser's exact code path) and seals a reply
// that this program then opens. Both directions must round-trip.
//
// Run: cargo run --example crypto_interop            (prints dev->web envelope)
//      cargo run --example crypto_interop -- open <nonce_b64> <ct_b64>
use aygent_lib::remote::CHUNK_BYTES;
use base64::Engine;
use crypto_box::aead::{Aead, AeadCore, OsRng};
use crypto_box::{ChaChaBox, PublicKey, SecretKey};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// Fixed test keys (TEST ONLY): dev secret = 0x01*32, web secret = 0x02*32.
fn keys() -> (SecretKey, SecretKey) {
    (SecretKey::from([1u8; 32]), SecretKey::from([2u8; 32]))
}

fn main() {
    let (dev_sk, web_sk) = keys();
    let dev_boxer = ChaChaBox::new(&web_sk.public_key(), &dev_sk);
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 4 && args[1] == "open" {
        // Open a web-sealed ciphertext (proves js -> rust).
        let nonce_bytes = B64.decode(&args[2]).expect("nonce b64");
        let ct = B64.decode(&args[3]).expect("ct b64");
        let nonce = crypto_box::aead::generic_array::GenericArray::from_slice(&nonce_bytes);
        match dev_boxer.decrypt(nonce, ct.as_slice()) {
            Ok(plain) => println!("RUST_OPENED: {}", String::from_utf8_lossy(&plain)),
            Err(_) => { println!("RUST_OPEN_FAILED"); std::process::exit(1); }
        }
        return;
    }

    // Seal a message (proves rust -> js once the JS side opens it).
    let msg = b"interop:rust->js:hello";
    let nonce = ChaChaBox::generate_nonce(&mut OsRng);
    let ct = dev_boxer.encrypt(&nonce, &msg[..]).expect("seal");
    println!(
        "{}",
        serde_json::json!({
            "dev_pub": B64.encode(dev_sk.public_key().as_bytes()),
            "web_pub": B64.encode(web_sk.public_key().as_bytes()),
            "nonce": B64.encode(nonce),
            "ct": B64.encode(ct),
            "chunk_bytes": CHUNK_BYTES,
        })
    );
}
