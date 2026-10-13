#![forbid(unsafe_code)]

use avm_contract_sdk::{
    AccountAddress, BatchContract, BatchInput, BatchOutput, Caller, CanonicalMessage, Contract,
    EventField, EventValue, MessageInput, MessageOutput, StateOutputMode, StatePatch, StateUpdate,
};
use avm_revenue_core::*;

pub const STATE_BYTES: usize = 65_540;
pub const RECORD_BASE: usize = 2052;
pub const RECORD_BYTES: usize = 32;
pub const E_AUDIT_ARGS: u32 = 3200;
pub const E_AUDIT_AUTH: u32 = 3201;
pub const E_AUDIT_REPLAY: u32 = 3202;
pub const E_AUDIT_OVERFLOW: u32 = 3203;
pub const MAX_BATCH: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub last_nonce: u64,
    pub total: u128,
    pub count: u64,
}
impl Record {
    pub fn decode(b: &[u8]) -> HubResult<Self> {
        if b.len() != RECORD_BYTES {
            return Err(E_AUDIT_ARGS);
        }
        Ok(Self { last_nonce: u64_at(b, 0)?, total: u128_at(b, 8)?, count: u64_at(b, 24)? })
    }
    pub fn encode(self) -> Vec<u8> {
        let mut b = self.last_nonce.to_le_bytes().to_vec();
        b.extend_from_slice(&self.total.to_le_bytes());
        b.extend_from_slice(&self.count.to_le_bytes());
        b
    }
}

pub struct AuditLedger;
impl Contract for AuditLedger {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        isolated(input).unwrap_or_else(MessageOutput::failure)
    }
}
fn initialize(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let Caller::External(owner) = input.caller else {
        return Err(E_AUDIT_AUTH);
    };
    let init = deploy_data(input.message.body())?;
    if init.len() != 1
        || init[0] == 0
        || init[0] > 16
        || !input.state.is_empty()
        || input.message.value_main() != 0
        || !input.message.extra().is_empty()
    {
        return Err(E_AUDIT_ARGS);
    }
    let mut b = vec![0; STATE_BYTES];
    b[..4].copy_from_slice(&65_536u32.to_le_bytes());
    b[4..36].copy_from_slice(&owner);
    b[36] = init[0];
    Ok(MessageOutput::success(StateUpdate::FullReplace(b)))
}

fn validate(b: &[u8]) -> HubResult<usize> {
    if b.len() != STATE_BYTES
        || b[..4] != 65_536u32.to_le_bytes()
        || b[36] == 0
        || b[36] > 16
        || b[37..68] != [0; 31]
        || b[4..36] == [0; 32]
    {
        return Err(E_AUDIT_ARGS);
    }
    let count = usize::from(b[36]);
    // Inactive binding/record slots and reserved retained payload stay canonical.
    if b[68 + count * 64..RECORD_BASE].iter().any(|v| *v != 0)
        || b[RECORD_BASE + count * RECORD_BYTES..].iter().any(|v| *v != 0)
    {
        return Err(E_AUDIT_ARGS);
    }
    let mut sources = Vec::new();
    for i in 0..count {
        let source = address_at(b, 68 + i * 64)?;
        if source != zero_address() {
            if !nonzero_address(source) || sources.contains(&source) {
                return Err(E_AUDIT_ARGS);
            }
            sources.push(source);
        }
        let r = record(b, i)?;
        if (r.count == 0 && r != Record::default())
            || (r.count != 0 && (r.last_nonce == 0 || r.total == 0))
        {
            return Err(E_AUDIT_ARGS);
        }
    }
    Ok(count)
}
pub fn record(b: &[u8], index: usize) -> HubResult<Record> {
    if index >= MAX_MEMBERS {
        return Err(E_AUDIT_ARGS);
    }
    b.get(RECORD_BASE + index * 32..RECORD_BASE + (index + 1) * 32)
        .ok_or(E_AUDIT_ARGS)
        .and_then(Record::decode)
}
fn patch(index: usize, r: Record) -> StatePatch {
    StatePatch {
        offset: (RECORD_BASE + index * RECORD_BYTES) as u32,
        old_len: RECORD_BYTES as u32,
        new_bytes: r.encode(),
    }
}
fn replacement(b: &[u8], mode: StateOutputMode, patches: Vec<StatePatch>) -> StateUpdate {
    if mode == StateOutputMode::DataPatchList {
        return StateUpdate::DataPatchList(patches);
    }
    let mut next = b.to_vec();
    for p in patches {
        let o = p.offset as usize;
        next[o..o + p.new_bytes.len()].copy_from_slice(&p.new_bytes);
    }
    StateUpdate::FullReplace(next)
}
fn audit_event(
    input_index: u32,
    index: usize,
    nonce: u64,
    amount: u128,
    r: Record,
) -> HubResult<avm_contract_sdk::EventIntent> {
    event(
        input_index,
        "hub.audited",
        vec![
            EventField { name: "amount".into(), value: EventValue::U128(amount) },
            EventField { name: "member".into(), value: EventValue::U64(index as u64) },
            EventField { name: "nonce".into(), value: EventValue::U64(nonce) },
            EventField { name: "receipts".into(), value: EventValue::U64(r.count) },
            EventField { name: "total".into(), value: EventValue::U128(r.total) },
        ],
    )
}

fn accepted(
    b: &[u8],
    count: usize,
    source: AccountAddress,
    message: CanonicalMessage<'_>,
    records: &mut [Record; MAX_MEMBERS],
) -> HubResult<(usize, u64, u128, Record)> {
    let CanonicalMessage::Internal(m) = message else {
        return Err(E_AUDIT_AUTH);
    };
    if m.method_id != ids::AUDIT_RECORD
        || m.is_callback()
        || m.is_deploy()
        || m.has_callback_request()
        || m.event_reads.is_some()
        || m.value_main != 0
        || !m.extra.is_empty()
    {
        return Err(E_AUDIT_ARGS);
    }
    let (index, nonce, amount) = read_payment(m.body)?;
    if index >= count || source != address_at(b, 68 + index * 64)? {
        return Err(E_AUDIT_AUTH);
    }
    let old = records[index];
    if nonce <= old.last_nonce {
        return Err(E_AUDIT_REPLAY);
    }
    let r = Record {
        last_nonce: nonce,
        total: old.total.checked_add(amount).ok_or(E_AUDIT_OVERFLOW)?,
        count: old.count.checked_add(1).ok_or(E_AUDIT_OVERFLOW)?,
    };
    records[index] = r;
    Ok((index, nonce, amount, r))
}

fn isolated(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let count = validate(input.state)?;
    let method = input.message.method_id();
    if method == ids::AUDIT_STATUS {
        let Caller::External(_) = input.caller else {
            return Err(E_AUDIT_AUTH);
        };
        no_body(input.message.body())?;
        if input.message.value_main() != 0 || !input.message.extra().is_empty() {
            return Err(E_AUDIT_ARGS);
        }
        return Ok(MessageOutput::success(replacement(
            input.state,
            input.required_state_output_mode,
            vec![],
        )));
    }
    if method == ids::AUDIT_BIND {
        if input.caller != Caller::External(array(input.state, 4)?) {
            return Err(E_AUDIT_AUTH);
        }
        let body = input.message.body();
        if body.len() != 65
            || usize::from(body[0]) >= count
            || input.message.value_main() != 0
            || !input.message.extra().is_empty()
        {
            return Err(E_AUDIT_ARGS);
        }
        let index = usize::from(body[0]);
        let source = address_at(body, 1)?;
        if !nonzero_address(source)
            || source.dapp_id != input.destination.dapp_id
            || source == input.destination
            || address_at(input.state, 68 + index * 64)? != zero_address()
        {
            return Err(E_AUDIT_ARGS);
        }
        for i in 0..count {
            if address_at(input.state, 68 + i * 64)? == source {
                return Err(E_AUDIT_ARGS);
            }
        }
        let p = StatePatch {
            offset: (68 + index * 64) as u32,
            old_len: 64,
            new_bytes: body[1..].to_vec(),
        };
        return Ok(MessageOutput::success(replacement(
            input.state,
            input.required_state_output_mode,
            vec![p],
        )));
    }
    let Caller::Internal(source) = input.caller else {
        return Err(E_AUDIT_AUTH);
    };
    let mut records = [Record::default(); MAX_MEMBERS];
    for (i, r) in records[..count].iter_mut().enumerate() {
        *r = record(input.state, i)?;
    }
    let (index, nonce, amount, r) =
        accepted(input.state, count, source, input.message, &mut records)?;
    let mut out = MessageOutput::success(replacement(
        input.state,
        input.required_state_output_mode,
        vec![patch(index, r)],
    ));
    out.event_intents.push(audit_event(0, index, nonce, amount, r)?);
    Ok(out)
}

impl BatchContract for AuditLedger {
    fn process_batch(input: BatchInput<'_>) -> BatchOutput {
        let failure = |code| BatchOutput {
            state_update: StateUpdate::DataPatchList(vec![]),
            status_codes: vec![code; input.message_count() as usize],
            outbound_intents: vec![],
            event_intents: vec![],
        };
        let Ok(count) = validate(input.state) else {
            return failure(E_AUDIT_ARGS);
        };
        if input.required_state_output_mode != StateOutputMode::DataPatchList
            || input.message_count() as usize > MAX_BATCH
        {
            return failure(E_AUDIT_ARGS);
        }
        let mut records = [Record::default(); MAX_MEMBERS];
        for (i, r) in records[..count].iter_mut().enumerate() {
            let Ok(value) = record(input.state, i) else {
                return failure(E_AUDIT_ARGS);
            };
            *r = value;
        }
        let mut touched = [false; MAX_MEMBERS];
        let mut statuses = Vec::new();
        let mut events = Vec::new();
        for (input_index, message) in input.messages().enumerate() {
            let result = match message {
                Ok(CanonicalMessage::Internal(m)) if m.destination == input.destination => {
                    accepted(
                        input.state,
                        count,
                        m.source,
                        CanonicalMessage::Internal(m),
                        &mut records,
                    )
                }
                _ => Err(E_AUDIT_AUTH),
            };
            match result {
                Ok((index, nonce, amount, r)) => {
                    match audit_event(input_index as u32, index, nonce, amount, r) {
                        Ok(e) => {
                            touched[index] = true;
                            statuses.push(0);
                            events.push(e);
                        }
                        Err(code) => return failure(code),
                    }
                }
                Err(code) => statuses.push(code),
            }
        }
        let patches = touched
            .iter()
            .enumerate()
            .filter(|(_, v)| **v)
            .map(|(i, _)| patch(i, records[i]))
            .collect();
        BatchOutput {
            state_update: StateUpdate::DataPatchList(patches),
            status_codes: statuses,
            outbound_intents: vec![],
            event_intents: events,
        }
    }
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_batch_contract!(AuditLedger, arena_bytes = 524_288);
