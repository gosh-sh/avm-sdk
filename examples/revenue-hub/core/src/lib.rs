#![forbid(unsafe_code)]

use avm_contract_sdk::{encode_event_fields, AccountAddress, EventField, EventIntent, EventKind};
pub mod ids;
pub mod vault;

pub type HubResult<T> = Result<T, u32>;
pub const MAX_MEMBERS: usize = 16;
pub const DENOMINATOR: u128 = 10_000;
pub const E_ARGS: u32 = 3000;
pub const E_AUTH: u32 = 3001;
pub const E_STATE: u32 = 3002;
pub const E_PAUSED: u32 = 3003;
pub const E_VALUE: u32 = 3004;
pub const E_BUSY: u32 = 3005;
pub const E_EMPTY: u32 = 3006;
pub const E_CALLBACK: u32 = 3007;
pub const E_OVERFLOW: u32 = 3008;
pub const E_BALANCE: u32 = 3009;
pub const E_GOVERNANCE: u32 = 3010;
pub const E_DUPLICATE_VOTE: u32 = 3011;
pub const E_TIMELOCK: u32 = 3012;
pub const E_EXPIRED: u32 = 3013;
pub const E_QUORUM: u32 = 3014;

pub fn array<const N: usize>(b: &[u8], offset: usize) -> HubResult<[u8; N]> {
    let end = offset.checked_add(N).ok_or(E_ARGS)?;
    b.get(offset..end).and_then(|v| v.try_into().ok()).ok_or(E_ARGS)
}
pub fn u128_at(b: &[u8], o: usize) -> HubResult<u128> {
    array(b, o).map(u128::from_le_bytes)
}
pub fn u64_at(b: &[u8], o: usize) -> HubResult<u64> {
    array(b, o).map(u64::from_le_bytes)
}
pub fn u32_at(b: &[u8], o: usize) -> HubResult<u32> {
    array(b, o).map(u32::from_le_bytes)
}
pub fn address_at(b: &[u8], o: usize) -> HubResult<AccountAddress> {
    Ok(AccountAddress { dapp_id: array(b, o)?, account_id: array(b, o + 32)? })
}
pub fn put_address(b: &mut [u8], o: usize, a: AccountAddress) {
    b[o..o + 32].copy_from_slice(&a.dapp_id);
    b[o + 32..o + 64].copy_from_slice(&a.account_id);
}
pub fn nonzero_address(a: AccountAddress) -> bool {
    a.dapp_id != [0; 32] && a.account_id != [0; 32]
}
pub fn zero_address() -> AccountAddress {
    AccountAddress { dapp_id: [0; 32], account_id: [0; 32] }
}
pub fn no_body(b: &[u8]) -> HubResult<()> {
    if b.is_empty() {
        Ok(())
    } else {
        Err(E_ARGS)
    }
}

/// floor(total * weight / 10,000), without overflowing total * weight.
pub fn entitlement(total: u128, weight: u16) -> HubResult<u128> {
    if weight == 0 || u128::from(weight) > DENOMINATOR {
        return Err(E_ARGS);
    }
    let w = u128::from(weight);
    (total / DENOMINATOR)
        .checked_mul(w)
        .and_then(|whole| whole.checked_add((total % DENOMINATOR) * w / DENOMINATOR))
        .ok_or(E_OVERFLOW)
}

pub fn payment_body(index: u8, nonce: u64, amount: u128) -> Vec<u8> {
    let mut b = vec![index];
    b.extend_from_slice(&nonce.to_le_bytes());
    b.extend_from_slice(&amount.to_le_bytes());
    b
}
pub fn read_payment(b: &[u8]) -> HubResult<(usize, u64, u128)> {
    if b.len() != 25 || usize::from(b[0]) >= MAX_MEMBERS {
        return Err(E_ARGS);
    }
    let nonce = u64_at(b, 1)?;
    let amount = u128_at(b, 9)?;
    if nonce == 0 || amount == 0 {
        return Err(E_ARGS);
    }
    Ok((usize::from(b[0]), nonce, amount))
}

/// Bounded extractor for the ACDP v1 envelope admitted by the pinned AVM runtime.
pub fn deploy_data(b: &[u8]) -> HubResult<&[u8]> {
    if b.len() < 288 || &b[..4] != b"ACDP" || b[6..8] != [0; 2] || b[284..288] != [0; 4] {
        return Err(E_ARGS);
    }
    let name = usize::from(u16::from_le_bytes(array(b, 264)?));
    let salt = usize::from(u16::from_le_bytes(array(b, 266)?));
    let start = 288usize.checked_add(name).and_then(|v| v.checked_add(salt)).ok_or(E_ARGS)?;
    let mut end = start;
    for o in [268, 272, 276, 280] {
        end = end.checked_add(u32_at(b, o)? as usize).ok_or(E_ARGS)?;
    }
    if end != b.len() {
        return Err(E_ARGS);
    }
    b.get(start..start.checked_add(u32_at(b, 268)? as usize).ok_or(E_ARGS)?).ok_or(E_ARGS)
}

pub fn event(input_index: u32, name: &str, mut fields: Vec<EventField>) -> HubResult<EventIntent> {
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    let class = match name {
        "hub.created" => ids::EVENT_CREATED,
        "hub.deposited" => ids::EVENT_DEPOSITED,
        "hub.claim_pending" => ids::EVENT_CLAIM_PENDING,
        "hub.claim_settled" => ids::EVENT_CLAIM_SETTLED,
        "hub.claim_failed" => ids::EVENT_CLAIM_FAILED,
        "hub.proposed" => ids::EVENT_PROPOSED,
        "hub.voted" => ids::EVENT_VOTED,
        "hub.executed" => ids::EVENT_EXECUTED,
        "hub.status" => ids::EVENT_STATUS,
        "hub.fees_added" => ids::EVENT_FEES_ADDED,
        "hub.audited" => ids::EVENT_AUDITED,
        _ => return Err(E_ARGS),
    };
    Ok(EventIntent {
        input_index,
        kind: EventKind::External,
        event_class: class,
        event_name: name.into(),
        topics: vec![],
        fields_binary: encode_event_fields(&fields).map_err(|_| E_STATE)?,
    })
}
