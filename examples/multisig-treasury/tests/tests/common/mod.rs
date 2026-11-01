//! A small deterministic stand for the treasury: it builds the canonical ACMI
//! frames the runtime would, applies accepted outputs, tracks the contract's real
//! ECC balance (including value that left in payouts and prepaid FUEL) and
//! delivers payee callbacks. It is test scaffolding, not a runtime model.

#![allow(dead_code)]

use std::collections::BTreeMap;

use avm_contract_sdk::{
    decode_message_input, encode_message_output, AccountAddress, Contract, MessageOutput,
    StateUpdate,
};
use avm_multisig_treasury::{ids, State, Treasury, SHELL_CURRENCY_ID};

pub const OWNER_A: [u8; 32] = [0x51; 32];
pub const OWNER_B: [u8; 32] = [0x52; 32];
pub const OWNER_C: [u8; 32] = [0x53; 32];
pub const OWNER_D: [u8; 32] = [0x54; 32];
pub const OUTSIDER: [u8; 32] = [0x6d; 32];
pub const C7: u32 = 7;
pub const C9: u32 = 9;
pub const FUEL: u128 = 10_000;
pub const DELAY: u32 = 3_600;
pub const GRACE: u32 = 86_400;
pub const TTL: u32 = 172_800;

pub fn addr(id: u8) -> AccountAddress {
    AccountAddress { dapp_id: [0x11; 32], account_id: [id; 32] }
}
pub const POOL: u8 = 1;

pub fn target_of(address: AccountAddress) -> [u8; 64] {
    let mut target = [0; 64];
    target[..32].copy_from_slice(&address.dapp_id);
    target[32..].copy_from_slice(&address.account_id);
    target
}
pub fn key_target(key: [u8; 32]) -> [u8; 64] {
    let mut target = [0; 64];
    target[..32].copy_from_slice(&key);
    target
}

/// Constructor bytes: `threshold | owners | delay | grace | ttl | fuel | keys`.
pub fn init_data(
    threshold: u8,
    owners: &[[u8; 32]],
    delay: u32,
    grace: u32,
    ttl: u32,
    fuel: u128,
) -> Vec<u8> {
    let mut bytes = vec![threshold, owners.len() as u8];
    bytes.extend_from_slice(&delay.to_le_bytes());
    bytes.extend_from_slice(&grace.to_le_bytes());
    bytes.extend_from_slice(&ttl.to_le_bytes());
    bytes.extend_from_slice(&fuel.to_le_bytes());
    for owner in owners {
        bytes.extend_from_slice(owner);
    }
    bytes
}
pub fn default_init(threshold: u8, owners: &[[u8; 32]]) -> Vec<u8> {
    init_data(threshold, owners, DELAY, GRACE, TTL, FUEL)
}

// ------------------------------------------------------------ wire frames ---

fn put_extras(bytes: &mut Vec<u8>, entries: &[(u32, u128)]) {
    for (id, amount) in entries {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
    }
}

fn write_address(bytes: &mut [u8], offset: usize, address: AccountAddress) {
    bytes[offset..offset + 32].copy_from_slice(&address.dapp_id);
    bytes[offset + 32..offset + 64].copy_from_slice(&address.account_id);
}

pub fn envelope(init: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 288];
    bytes[..4].copy_from_slice(b"ACDP");
    bytes[268..272].copy_from_slice(&(init.len() as u32).to_le_bytes());
    bytes.extend_from_slice(init);
    bytes
}

pub fn external_message(
    body: &[u8],
    actor: [u8; 32],
    method: u64,
    extra: &[(u32, u128)],
    destination: AccountAddress,
    deploy: bool,
) -> Vec<u8> {
    let mut bytes = vec![0; 192];
    bytes[0] = 1;
    bytes[1] = if deploy { 3 } else { 1 };
    bytes[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&method.to_le_bytes());
    bytes[16..32].copy_from_slice(&10_000u128.to_le_bytes());
    bytes[32..40].copy_from_slice(&1u64.to_le_bytes());
    bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    write_address(&mut bytes, 48, destination);
    bytes[112..144].copy_from_slice(&actor);
    bytes[144..146].copy_from_slice(&64u16.to_le_bytes());
    put_extras(&mut bytes, extra);
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&[0; 64]);
    bytes
}

/// A canonical internal message. `callback` is `(cause hash, status)` for a
/// native callback; `handler` is the callback method requested by a sender.
#[allow(clippy::too_many_arguments)]
pub fn internal_message(
    method: u64,
    body: &[u8],
    extra: &[(u32, u128)],
    source: AccountAddress,
    destination: AccountAddress,
    callback: Option<([u8; 32], u32)>,
    handler: Option<u64>,
    fuel_fee: u128,
) -> Vec<u8> {
    let mut bytes = vec![0; 176];
    bytes[1] = u8::from(callback.is_some()) | (u8::from(handler.is_some()) << 2);
    bytes[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&method.to_le_bytes());
    bytes[16..32].copy_from_slice(&fuel_fee.to_le_bytes());
    bytes[32..40].copy_from_slice(&5u64.to_le_bytes());
    bytes[40..44].copy_from_slice(&20u32.to_le_bytes());
    write_address(&mut bytes, 48, source);
    write_address(&mut bytes, 112, destination);
    if let Some((hash, status)) = callback {
        bytes.extend_from_slice(&hash);
        bytes.extend_from_slice(&destination.dapp_id);
        bytes.extend_from_slice(&destination.account_id);
        bytes.extend_from_slice(&status.to_le_bytes());
    }
    if let Some(handler) = handler {
        bytes.extend_from_slice(&handler.to_le_bytes());
    }
    put_extras(&mut bytes, extra);
    bytes.extend_from_slice(body);
    bytes
}

/// The ACMI execution frame. `external` is `Some(pubkey)` for a signed call,
/// otherwise the message must be internal and its source becomes the caller.
pub fn frame(
    message: &[u8],
    state: &[u8],
    external: Option<[u8; 32]>,
    source: Option<AccountAddress>,
    destination: AccountAddress,
    time: u32,
    balances: &BTreeMap<u32, u128>,
) -> Vec<u8> {
    let mut bytes = Vec::from(*b"ACMI");
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&time.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&[0x12; 32]);
    bytes.push(u8::from(external.is_some()));
    bytes.extend_from_slice(&[0; 7]);
    match (external, source) {
        (None, Some(source)) => {
            bytes.extend_from_slice(&source.dapp_id);
            bytes.extend_from_slice(&source.account_id);
            bytes.extend_from_slice(&[0; 32]);
        }
        (Some(actor), _) => {
            bytes.extend_from_slice(&[0; 64]);
            bytes.extend_from_slice(&actor);
        }
        (None, None) => panic!("an internal frame needs a source"),
    }
    bytes.extend_from_slice(&destination.dapp_id);
    bytes.extend_from_slice(&destination.account_id);
    bytes.extend_from_slice(&[0x44; 32]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&(balances.len() as u16).to_le_bytes());
    for (id, amount) in balances {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
    }
    bytes.extend_from_slice(&(state.len() as u32).to_le_bytes());
    bytes.extend_from_slice(state);
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(message);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}

// ------------------------------------------------------------- the stand ---

/// A payout the treasury emitted, waiting for a callback.
#[derive(Clone, Debug)]
pub struct Dispatched {
    pub recipient: AccountAddress,
    pub extra: Vec<(u32, u128)>,
    pub body: Vec<u8>,
    pub method: u64,
    pub fuel: u128,
    pub handler: u64,
}

pub enum Settle {
    /// The payee accepted and echoed this body.
    Delivered(Vec<u8>),
    /// Accepted, but the callback also (wrongly) carries these entries back.
    DeliveredReturning(Vec<u8>, Vec<(u32, u128)>),
    /// Delivery failed; these entries come back with the callback.
    Bounced(Vec<(u32, u128)>),
}

pub struct Sim {
    pub state: Vec<u8>,
    /// The contract's physical ECC balance.
    pub vault: BTreeMap<u32, u128>,
    pub time: u32,
    pub outbox: Vec<Dispatched>,
    pub next_hash: u8,
    pub init_events: Vec<String>,
    pub events: Vec<String>,
}

fn encode_checked(output: &MessageOutput) {
    encode_message_output(output).expect("every returned effect set must be canonical");
}

pub fn accepted(output: &MessageOutput) {
    assert_eq!(output.status_code, 0, "expected success");
}

pub fn failure(output: &MessageOutput, code: u32) {
    assert_eq!(output.status_code, code, "wrong error code");
    assert!(output.state_update.is_none(), "a failure must not update state");
    assert!(
        output.outbound_intents.is_empty()
            && output.callback_intents.is_empty()
            && output.event_intents.is_empty(),
        "a failure must not carry effects"
    );
}

fn names(output: &MessageOutput) -> Vec<String> {
    output.event_intents.iter().map(|event| event.event_name.clone()).collect()
}

impl Sim {
    /// Deploys with `init` as the constructor bytes, signed by `deployer`.
    pub fn try_deploy(init: &[u8], deployer: [u8; 32]) -> Result<Self, Box<MessageOutput>> {
        let message = external_message(&envelope(init), deployer, 0, &[], addr(POOL), true);
        let framed = frame(&message, &[], Some(deployer), None, addr(POOL), 10, &BTreeMap::new());
        let output = Treasury::init(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code != 0 {
            return Err(Box::new(output));
        }
        let Some(StateUpdate::FullReplace(state)) = output.state_update.clone() else {
            panic!("init must return the full state")
        };
        Ok(Self {
            state,
            vault: BTreeMap::new(),
            time: 1_000,
            outbox: Vec::new(),
            next_hash: 1,
            init_events: names(&output),
            events: Vec::new(),
        })
    }

    /// A 2-of-3 treasury owned by A, B and C.
    pub fn new() -> Self {
        Self::with(2, &[OWNER_A, OWNER_B, OWNER_C])
    }

    pub fn with(threshold: u8, owners: &[[u8; 32]]) -> Self {
        Self::try_deploy(&default_init(threshold, owners), owners[0]).expect("deploys")
    }

    /// A funded treasury: 1_000_000 of C7 and C9 plus SHELL for FUEL.
    pub fn funded(threshold: u8, owners: &[[u8; 32]]) -> Self {
        let mut sim = Self::with(threshold, owners);
        accepted(
            &sim.deposit(
                OWNER_A,
                &[(SHELL_CURRENCY_ID, 1_000_000), (C7, 1_000_000), (C9, 1_000_000)],
            ),
        );
        sim
    }

    pub fn state(&self) -> State {
        State::decode(&self.state).expect("a committed state always decodes")
    }

    pub fn balance(&self, currency: u32) -> u128 {
        self.vault.get(&currency).copied().unwrap_or(0)
    }

    pub fn advance(&mut self, seconds: u32) {
        self.time += seconds;
    }

    fn commit(&mut self, output: &MessageOutput, balances: BTreeMap<u32, u128>) {
        let Some(StateUpdate::FullReplace(state)) = &output.state_update else {
            panic!("an accepted call returns the full state")
        };
        self.state = state.clone();
        self.vault = balances;
        self.events = names(output);
        for intent in &output.outbound_intents {
            for entry in &intent.extra {
                let held = self.vault.get_mut(&entry.currency_id).expect("vault balance");
                *held = held.checked_sub(entry.amount).expect("payout must be covered");
            }
            // The sender prepays the FUEL reserve when the message is materialized.
            let shell = self.vault.get_mut(&SHELL_CURRENCY_ID).expect("SHELL balance");
            *shell = shell.checked_sub(intent.fuel_fee_shells).expect("FUEL must be covered");
            let handler = output
                .callback_intents
                .iter()
                .find(|callback| callback.outbound_index == 0)
                .map(|callback| callback.handler_method_id)
                .expect("payouts request a callback");
            self.outbox.push(Dispatched {
                recipient: intent.destination,
                extra: intent.extra.iter().map(|e| (e.currency_id, e.amount)).collect(),
                body: intent.body.clone(),
                method: intent.method_id,
                fuel: intent.fuel_fee_shells,
                handler,
            });
        }
        self.vault.retain(|_, amount| *amount != 0);
    }

    /// Sends a signed external call. Accepted calls commit state and value.
    pub fn call(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: &[u8],
        extra: &[(u32, u128)],
    ) -> MessageOutput {
        let message = external_message(body, actor, method, extra, addr(POOL), false);
        let mut balances = self.vault.clone();
        for (id, amount) in extra {
            *balances.entry(*id).or_insert(0) += amount;
        }
        let framed =
            frame(&message, &self.state, Some(actor), None, addr(POOL), self.time, &balances);
        let output = Treasury::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            self.commit(&output, balances);
        }
        output
    }

    /// Same as [`Self::call`] but frames the balance as it was *before* the
    /// message's own value arrived.
    pub fn call_with_pre_credit_balance(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: &[u8],
        extra: &[(u32, u128)],
    ) -> MessageOutput {
        let message = external_message(body, actor, method, extra, addr(POOL), false);
        let framed =
            frame(&message, &self.state, Some(actor), None, addr(POOL), self.time, &self.vault);
        let output = Treasury::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            let mut balances = self.vault.clone();
            for (id, amount) in extra {
                *balances.entry(*id).or_insert(0) += amount;
            }
            self.commit(&output, balances);
        }
        output
    }

    /// A deposit from another contract (an internal message).
    pub fn internal_deposit(
        &mut self,
        source: AccountAddress,
        extra: &[(u32, u128)],
    ) -> MessageOutput {
        let message = internal_message(ids::DEPOSIT, &[], extra, source, addr(POOL), None, None, 0);
        let mut balances = self.vault.clone();
        for (id, amount) in extra {
            *balances.entry(*id).or_insert(0) += amount;
        }
        let framed =
            frame(&message, &self.state, None, Some(source), addr(POOL), self.time, &balances);
        let output = Treasury::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            self.commit(&output, balances);
        }
        output
    }

    pub fn fresh_hash(&mut self) -> [u8; 32] {
        let hash = [self.next_hash; 32];
        self.next_hash += 1;
        hash
    }

    /// Delivers the payee's callback for the pending payout to `recipient`.
    pub fn settle(&mut self, recipient: AccountAddress, how: Settle) -> MessageOutput {
        let hash = self.fresh_hash();
        self.settle_with(recipient, how, hash, ids::CALLBACK, recipient)
    }

    /// A callback with full control over its header, for negative tests.
    pub fn settle_with(
        &mut self,
        recipient: AccountAddress,
        how: Settle,
        hash: [u8; 32],
        method: u64,
        source: AccountAddress,
    ) -> MessageOutput {
        let (status, body, mut returned) = match how {
            Settle::Delivered(body) => (0, body, Vec::new()),
            Settle::DeliveredReturning(body, returned) => (0, body, returned),
            Settle::Bounced(returned) => (1500, Vec::new(), returned),
        };
        returned.sort();
        let message = internal_message(
            method,
            &body,
            &returned,
            source,
            addr(POOL),
            Some((hash, status)),
            None,
            0,
        );
        let mut balances = self.vault.clone();
        for (id, amount) in &returned {
            *balances.entry(*id).or_insert(0) += amount;
        }
        let framed =
            frame(&message, &self.state, None, Some(source), addr(POOL), self.time, &balances);
        let output = Treasury::process_message(decode_message_input(&framed).unwrap());
        encode_checked(&output);
        if output.status_code == 0 {
            self.commit(&output, balances);
            if let Some(index) = self.outbox.iter().position(|d| d.recipient == recipient) {
                self.outbox.remove(index);
            }
        }
        output
    }

    // ------------------------------------------------------ convenience ---

    pub fn deposit(&mut self, actor: [u8; 32], extra: &[(u32, u128)]) -> MessageOutput {
        let mut sorted = extra.to_vec();
        sorted.sort();
        self.call(actor, ids::DEPOSIT, &[], &sorted)
    }

    pub fn propose(
        &mut self,
        actor: [u8; 32],
        kind: u8,
        currency: u32,
        aux: u32,
        amount: u128,
        target: [u8; 64],
    ) -> MessageOutput {
        let mut body = vec![kind];
        body.extend_from_slice(&currency.to_le_bytes());
        body.extend_from_slice(&aux.to_le_bytes());
        body.extend_from_slice(&amount.to_le_bytes());
        body.extend_from_slice(&target);
        self.call(actor, ids::PROPOSE, &body, &[])
    }

    /// Proposes and returns the new proposal's id.
    pub fn propose_ok(
        &mut self,
        actor: [u8; 32],
        kind: u8,
        currency: u32,
        aux: u32,
        amount: u128,
        target: [u8; 64],
    ) -> u64 {
        accepted(&self.propose(actor, kind, currency, aux, amount, target));
        self.state().next_id - 1
    }

    pub fn transfer(
        &mut self,
        actor: [u8; 32],
        to: AccountAddress,
        currency: u32,
        amount: u128,
    ) -> u64 {
        self.propose_ok(
            actor,
            avm_multisig_treasury::KIND_TRANSFER,
            currency,
            0,
            amount,
            target_of(to),
        )
    }

    pub fn act(&mut self, actor: [u8; 32], method: u64, id: u64) -> MessageOutput {
        self.call(actor, method, &id.to_le_bytes(), &[])
    }
    pub fn approve(&mut self, actor: [u8; 32], id: u64) -> MessageOutput {
        self.act(actor, ids::APPROVE, id)
    }
    pub fn cancel(&mut self, actor: [u8; 32], id: u64) -> MessageOutput {
        self.act(actor, ids::CANCEL, id)
    }
    pub fn execute(&mut self, actor: [u8; 32], id: u64) -> MessageOutput {
        self.act(actor, ids::EXECUTE, id)
    }
    pub fn freeze(&mut self, actor: [u8; 32]) -> MessageOutput {
        self.call(actor, ids::FREEZE, &[], &[])
    }

    /// Approves with the first two owners other than the proposer's and runs the
    /// whole lifecycle for a governance proposal: approve, wait, execute.
    pub fn pass(&mut self, id: u64, approvers: &[[u8; 32]]) -> MessageOutput {
        for approver in approvers {
            let state = self.state();
            let proposal = state.proposals[state.proposal_index(id).expect("exists")];
            let slot = state.owner_slot(approver).expect("owner");
            if proposal.approvals & (1 << slot) == 0 {
                accepted(&self.approve(*approver, id));
            }
        }
        let proposal = self.state().proposals[self.state().proposal_index(id).unwrap()];
        assert_eq!(proposal.status, avm_multisig_treasury::STATUS_QUEUED, "should be queued");
        self.advance(proposal.eta.saturating_sub(self.time));
        self.execute(approvers[0], id)
    }

    /// The accounting identity: the contract physically holds at least what it
    /// believes it owns, per currency.
    pub fn assert_accounting(&self) {
        let state = self.state();
        for entry in &state.balances {
            assert!(
                self.balance(entry.currency) >= entry.held,
                "vault holds {} of currency {} but the ledger says {}",
                self.balance(entry.currency),
                entry.currency,
                entry.held
            );
        }
        state.validate().expect("state invariants");
    }
}
