#![forbid(unsafe_code)]

use avm_contract_sdk::crypto;

#[test]
fn contract_crypto_public_types_are_safe_and_fixed() {
    let _: fn(&[u8]) -> [u8; 32] = crypto::blake3;
    let _: fn(&[u8]) -> [u8; 32] = crypto::sha256;
    let _: fn(&[u8; 32], &[u8; 64], &[u8]) -> bool = crypto::ed25519_verify;
}

#[test]
#[should_panic(expected = "AVM crypto helpers require WASM execution on a matching runtime")]
fn native_blake3_fails_explicitly() {
    crypto::blake3(b"abc");
}

#[test]
#[should_panic(expected = "AVM crypto helpers require WASM execution on a matching runtime")]
fn native_sha256_fails_explicitly() {
    crypto::sha256(b"abc");
}

#[test]
#[should_panic(expected = "AVM crypto helpers require WASM execution on a matching runtime")]
fn native_ed25519_fails_explicitly() {
    crypto::ed25519_verify(&[0; 32], &[0; 64], b"abc");
}
