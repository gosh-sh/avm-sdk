#![forbid(unsafe_code)]

//! A factory that deploys children and later accepts messages only from them.
//!
//! One package plays three roles, chosen by the constructor data:
//! - factory: `factory.spawn` deploys a child; `factory.accept` takes a report
//!   and checks that the internal sender's address derives from this package,
//!   this factory as owner, the reported salt and the child constructor data;
//! - child: `init` refuses any deployer other than the factory its constructor
//!   names; `child.report` sends its salt to that factory;
//! - relay: an ordinary account that forwards a message, or builds the same
//!   child deploy envelope a factory would and sends it itself.
//!
//! `prove.sh` runs the cases on a local stand. Address derivation proves the
//! package, owner, salt and constructor data of a sender, not the code version
//! it runs, and the owner field is not bound to the deployer, so the factory
//! cannot tell its own children from a child of another version of the same
//! package deployed by someone else.

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::Caller;
use avm_contract_sdk::Contract;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;

// Method ids are hash_u64_le of the names in avm-contract.toml.
const PING_METHOD_ID: u64 = 0xca77_f025_f934_3f7a;
const SPAWN_METHOD_ID: u64 = 0x7d46_88c3_7b01_7043;
const ACCEPT_METHOD_ID: u64 = 0x319b_6391_d476_ed41;
const REPORT_METHOD_ID: u64 = 0x2cbc_a295_1661_e9ac;
const SEND_METHOD_ID: u64 = 0x6110_8ed4_c55e_3864;
const FORGE_METHOD_ID: u64 = 0x860e_8543_373c_2373;

const INVALID_STATE: u32 = 1600;
const INVALID_INIT: u32 = 1601;
const WRONG_ROLE: u32 = 1602;
const INVALID_ARGS: u32 = 1603;
const INIT_CALLER_NOT_FACTORY: u32 = 1604;
const NOT_A_CHILD: u32 = 1605;
const UNKNOWN_METHOD: u32 = 1606;

/// Built with `PROVENANCE_PERMISSIVE_CHILD` set, the child `init` skips the
/// deployer check. `prove.sh` publishes that build as a second version of the
/// same package, as its package owner could.
const PERMISSIVE_CHILD: bool = option_env!("PROVENANCE_PERMISSIVE_CHILD").is_some();

const ROLE_FACTORY: u8 = 1;
const ROLE_CHILD: u8 = 2;
const ROLE_RELAY: u8 = 3;

// Constructor data: role u8 | factory address 64 (zero unless child).
const INIT_BYTES: usize = 65;
const SALT_BYTES: usize = 8;

// State layout:
//   [0]        role
//   [1]        1 when built as the permissive version
//   [8..16]    method id of the deploy message this account was created by
//   [16..112]  package id | WASM hash | descriptor hash, from its own envelope
//   factory: [112..116] accepted reports u32 | [116..180] last accepted sender
//            [180..244] last spawned child
//   child:   [112..176] factory | [176..240] init caller | [240..248] salt
const STATE_BYTES: usize = 256;
const TEMPLATE: core::ops::Range<usize> = 16..112;

// ACDP deployment envelope, 288-byte header: magic, flags u16, reserved u16,
// selected code id [8..40], package id [40..72], WASM hash [72..104],
// descriptor hash [104..136], owner id [136..168], BLAKE3 of the constructor
// data [168..200], owner key [200..232], ABI hash [232..264], then lengths of
// the UI name u16 [264], salt u16 [266], constructor data u32 [268] and of
// the inline code parts. The tail holds UI name, salt and constructor data.
const HEADER_BYTES: usize = 288;

struct FactoryChildProvenance;

impl Contract for FactoryChildProvenance {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let body = input.message.body();
        let (Some(header), Some(ui_len), Some(salt_len), Some(init_len)) = (
            body.get(..HEADER_BYTES),
            read_u16(body, 264),
            read_u16(body, 266),
            read_u32(body, 268),
        ) else {
            return MessageOutput::failure(INVALID_INIT);
        };
        let salt_at = HEADER_BYTES + ui_len;
        let init_at = salt_at + salt_len;
        let (Some(salt), Some(init)) = (
            body.get(salt_at..init_at),
            init_at.checked_add(init_len).and_then(|end| body.get(init_at..end)),
        ) else {
            return MessageOutput::failure(INVALID_INIT);
        };
        if init.len() != INIT_BYTES {
            return MessageOutput::failure(INVALID_INIT);
        }
        let mut state = vec![0; STATE_BYTES];
        state[0] = init[0];
        state[1] = u8::from(PERMISSIVE_CHILD);
        state[8..16].copy_from_slice(&input.message.method_id().to_le_bytes());
        state[TEMPLATE].copy_from_slice(&header[40..136]);
        match init[0] {
            ROLE_FACTORY | ROLE_RELAY if init[1..] == [0; 64] => {}
            ROLE_CHILD if salt.len() == SALT_BYTES => {
                let Caller::Internal(deployer) = input.caller else {
                    return MessageOutput::failure(INIT_CALLER_NOT_FACTORY);
                };
                if !PERMISSIVE_CHILD && address_bytes(deployer)[..] != init[1..] {
                    return MessageOutput::failure(INIT_CALLER_NOT_FACTORY);
                }
                state[112..176].copy_from_slice(&init[1..]);
                state[176..240].copy_from_slice(&address_bytes(deployer));
                state[240..248].copy_from_slice(salt);
            }
            _ => return MessageOutput::failure(INVALID_INIT),
        }
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.state.len() != STATE_BYTES {
            return MessageOutput::failure(INVALID_STATE);
        }
        let role = input.state[0];
        match (input.message.method_id(), role) {
            (PING_METHOD_ID, _) => unchanged(&input),
            (SPAWN_METHOD_ID, ROLE_FACTORY) => spawn(&input),
            (ACCEPT_METHOD_ID, ROLE_FACTORY) => accept(&input),
            (REPORT_METHOD_ID, ROLE_CHILD) => report(&input),
            (SEND_METHOD_ID, ROLE_RELAY) => relay_send(&input),
            (FORGE_METHOD_ID, ROLE_RELAY) => relay_forge(&input),
            (
                SPAWN_METHOD_ID | ACCEPT_METHOD_ID | REPORT_METHOD_ID | SEND_METHOD_ID
                | FORGE_METHOD_ID,
                _,
            ) => MessageOutput::failure(WRONG_ROLE),
            // Includes CODE_UPGRADE_METHOD_ID: no version of this package upgrades.
            _ => MessageOutput::failure(UNKNOWN_METHOD),
        }
    }
}

/// `factory.spawn(salt)`: deploys a child named after this factory.
fn spawn(input: &MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let Ok(salt) = <[u8; SALT_BYTES]>::try_from(input.message.body()) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let (child, envelope) = child_deploy(input, input.destination, &salt);
    let mut state = input.state.to_vec();
    state[180..244].copy_from_slice(&address_bytes(child));
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
    output.outbound_intents = vec![outbound(child, deploy_method_id(input), envelope)];
    output
}

/// `factory.accept(salt)`: counts a report only from an address that derives
/// from this package, this factory as owner, `salt` and the child init data.
fn accept(input: &MessageInput<'_>) -> MessageOutput {
    let Caller::Internal(sender) = input.caller else {
        return MessageOutput::failure(NOT_A_CHILD);
    };
    let Ok(salt) = <[u8; SALT_BYTES]>::try_from(input.message.body()) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let factory = input.destination;
    let expected = child_account_id(&input.state[TEMPLATE][..32], factory, &salt);
    if sender.dapp_id != factory.dapp_id || sender.account_id != expected {
        return MessageOutput::failure(NOT_A_CHILD);
    }
    let mut state = input.state.to_vec();
    let accepted = u32::from_le_bytes([state[112], state[113], state[114], state[115]]);
    state[112..116].copy_from_slice(&accepted.wrapping_add(1).to_le_bytes());
    state[116..180].copy_from_slice(&address_bytes(sender));
    MessageOutput::success(StateUpdate::FullReplace(state))
}

/// `child.report`: tells the factory named in the constructor data its salt.
fn report(input: &MessageInput<'_>) -> MessageOutput {
    let Some(factory) = address_from(&input.state[112..176]) else {
        return MessageOutput::failure(INVALID_STATE);
    };
    let mut output = unchanged(input);
    output.outbound_intents =
        vec![outbound(factory, ACCEPT_METHOD_ID, input.state[240..248].to_vec())];
    output
}

/// `relay.send(destination 64 | method id u64 | body)`.
fn relay_send(input: &MessageInput<'_>) -> MessageOutput {
    let Some(args) = bytes_arg(input.message.body()) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let (Some(destination), Some(method_id), Some(body)) =
        (args.get(..64).and_then(address_from), read_u64(args, 64), args.get(72..))
    else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let mut output = unchanged(input);
    output.outbound_intents = vec![outbound(destination, method_id, body.to_vec())];
    output
}

/// `relay.forge(factory 64 | salt 8)`: the child deploy that `factory.spawn`
/// would send, built and sent by this relay with its own code version.
fn relay_forge(input: &MessageInput<'_>) -> MessageOutput {
    let args = input.message.body();
    let (Some(factory), Some(salt)) = (
        args.get(..64).and_then(address_from),
        args.get(64..).and_then(|salt| <[u8; SALT_BYTES]>::try_from(salt).ok()),
    ) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let (child, envelope) = child_deploy(input, factory, &salt);
    let mut output = unchanged(input);
    output.outbound_intents = vec![outbound(child, deploy_method_id(input), envelope)];
    output
}

/// The address and deploy envelope of a child of `factory`, using the code
/// selection this account runs and the package fields of its own envelope.
fn child_deploy(
    input: &MessageInput<'_>,
    factory: AccountAddress,
    salt: &[u8; SALT_BYTES],
) -> (AccountAddress, Vec<u8>) {
    let init = child_init(factory);
    let mut envelope = vec![0; HEADER_BYTES];
    envelope[..4].copy_from_slice(b"ACDP");
    envelope[8..40].copy_from_slice(&input.selected_code_id);
    envelope[40..136].copy_from_slice(&input.state[TEMPLATE]);
    envelope[136..168].copy_from_slice(&factory.account_id);
    envelope[168..200].copy_from_slice(&blake3(&init));
    envelope[266..268].copy_from_slice(&(SALT_BYTES as u16).to_le_bytes());
    envelope[268..272].copy_from_slice(&(INIT_BYTES as u32).to_le_bytes());
    envelope.extend_from_slice(salt);
    envelope.extend_from_slice(&init);
    let child = AccountAddress {
        dapp_id: factory.dapp_id,
        account_id: child_account_id(&input.state[TEMPLATE][..32], factory, salt),
    };
    (child, envelope)
}

fn child_init(factory: AccountAddress) -> [u8; INIT_BYTES] {
    let mut init = [0; INIT_BYTES];
    init[0] = ROLE_CHILD;
    init[1..].copy_from_slice(&address_bytes(factory));
    init
}

/// blake3("AVM_ACCOUNT_ID" | u16 UI name length | UI name | package id |
/// owner id | u16 salt length | salt | blake3(constructor data)), with an
/// empty UI name and the factory as owner.
fn child_account_id(package_id: &[u8], factory: AccountAddress, salt: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(122);
    preimage.extend_from_slice(b"AVM_ACCOUNT_ID");
    preimage.extend_from_slice(&0u16.to_le_bytes());
    preimage.extend_from_slice(package_id);
    preimage.extend_from_slice(&factory.account_id);
    preimage.extend_from_slice(&(salt.len() as u16).to_le_bytes());
    preimage.extend_from_slice(salt);
    preimage.extend_from_slice(&blake3(&child_init(factory)));
    blake3(&preimage)
}

fn deploy_method_id(input: &MessageInput<'_>) -> u64 {
    u64::from_le_bytes(input.state[8..16].try_into().unwrap_or([0; 8]))
}

fn outbound(destination: AccountAddress, method_id: u64, body: Vec<u8>) -> OutboundIntent {
    OutboundIntent {
        input_index: 0,
        destination,
        value_main: 0,
        fuel_fee_shells: 10_000,
        extra: Vec::new(),
        method_id,
        body,
    }
}

fn unchanged(input: &MessageInput<'_>) -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
}

fn address_bytes(address: AccountAddress) -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(&address.dapp_id);
    bytes[32..].copy_from_slice(&address.account_id);
    bytes
}

fn address_from(bytes: &[u8]) -> Option<AccountAddress> {
    Some(AccountAddress {
        dapp_id: bytes.get(..32)?.try_into().ok()?,
        account_id: bytes.get(32..64)?.try_into().ok()?,
    })
}

/// An ABI `bytes` argument: u32 length prefix, then exactly that many bytes.
fn bytes_arg(body: &[u8]) -> Option<&[u8]> {
    let len = read_u32(body, 0)?;
    body.get(4..).filter(|args| args.len() == len)
}

fn read_u16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?)))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)).ok()
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

// BLAKE3 for inputs of at most one chunk (1024 bytes), which covers the
// account preimage and the constructor data here.
const IV: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];
const MSG_PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START: u32 = 1;
const CHUNK_END: u32 = 2;
const ROOT: u32 = 8;

fn blake3(input: &[u8]) -> [u8; 32] {
    let mut cv = IV;
    let blocks = input.len().div_ceil(64).max(1);
    for index in 0..blocks {
        let block = &input[(index * 64).min(input.len())..((index + 1) * 64).min(input.len())];
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks(4)) {
            let mut le = [0u8; 4];
            le[..bytes.len()].copy_from_slice(bytes);
            *word = u32::from_le_bytes(le);
        }
        let mut flags = if index == 0 { CHUNK_START } else { 0 };
        if index + 1 == blocks {
            flags |= CHUNK_END | ROOT;
        }
        cv = compress(&cv, words, block.len() as u32, flags);
    }
    let mut out = [0u8; 32];
    for (bytes, word) in out.chunks_mut(4).zip(cv) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn compress(cv: &[u32; 8], mut m: [u32; 16], block_len: u32, flags: u32) -> [u32; 8] {
    let mut s = [
        cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1], IV[2], IV[3], 0, 0,
        block_len, flags,
    ];
    for round in 0..7 {
        g(&mut s, [0, 4, 8, 12], m[0], m[1]);
        g(&mut s, [1, 5, 9, 13], m[2], m[3]);
        g(&mut s, [2, 6, 10, 14], m[4], m[5]);
        g(&mut s, [3, 7, 11, 15], m[6], m[7]);
        g(&mut s, [0, 5, 10, 15], m[8], m[9]);
        g(&mut s, [1, 6, 11, 12], m[10], m[11]);
        g(&mut s, [2, 7, 8, 13], m[12], m[13]);
        g(&mut s, [3, 4, 9, 14], m[14], m[15]);
        if round < 6 {
            m = MSG_PERMUTATION.map(|i| m[i]);
        }
    }
    core::array::from_fn(|i| s[i] ^ s[i + 8])
}

fn g(s: &mut [u32; 16], [a, b, c, d]: [usize; 4], x: u32, y: u32) {
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(x);
    s[d] = (s[d] ^ s[a]).rotate_right(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(12);
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(y);
    s[d] = (s[d] ^ s[a]).rotate_right(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(7);
}

avm_contract_sdk::export_message_contract!(FactoryChildProvenance, arena_bytes = 262_144);
