#![forbid(unsafe_code)]

//! Verifies a trusted signer's signature over SHA-256(payload), then stores
//! BLAKE3(payload). The external-call owner and the payload signer are separate.

use avm_contract_sdk::{crypto, Contract, MessageInput, MessageOutput, StateUpdate};

mod generated {
    include!(env!("AVM_CONTRACT_BINDINGS"));
}

struct SignatureVerifier;

impl Contract for SignatureVerifier {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Ok(deploy) = input.deploy() else {
            return MessageOutput::failure(generated::errors::INVALID_INPUT);
        };
        let Some(key) = bytes_argument(deploy.init_data()).filter(|key| key.len() == 32) else {
            return MessageOutput::failure(generated::errors::INVALID_INPUT);
        };
        let mut state = Vec::with_capacity(96);
        state.extend_from_slice(&deploy.owner_id());
        state.extend_from_slice(key);
        state.extend_from_slice(&[0; 32]);
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.message.method_id() != generated::methods::CRYPTO_VERIFY {
            return MessageOutput::failure(generated::errors::UNKNOWN_METHOD);
        }
        if input.state.len() != 96 {
            return MessageOutput::failure(generated::errors::INVALID_STATE);
        }
        let Ok(key) = <&[u8; 32]>::try_from(&input.state[32..64]) else {
            return MessageOutput::failure(generated::errors::INVALID_STATE);
        };
        let Some(args) = bytes_argument(input.message.body()) else {
            return MessageOutput::failure(generated::errors::INVALID_INPUT);
        };
        let Some(signature) = args.get(..64).and_then(|s| <&[u8; 64]>::try_from(s).ok()) else {
            return MessageOutput::failure(generated::errors::INVALID_INPUT);
        };
        let payload = &args[64..];
        let digest = crypto::sha256(payload);
        if !crypto::ed25519_verify(key, signature, &digest) {
            return MessageOutput::failure(generated::errors::INVALID_SIGNATURE);
        }
        let mut state = input.state[..64].to_vec();
        state.extend_from_slice(&crypto::blake3(payload));
        MessageOutput::success(StateUpdate::FullReplace(state))
    }
}

fn bytes_argument(body: &[u8]) -> Option<&[u8]> {
    let length = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
    body.get(4..).filter(|bytes| bytes.len() == length)
}

avm_contract_sdk::export_message_contract!(SignatureVerifier);
