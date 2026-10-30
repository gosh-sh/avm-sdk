#![forbid(unsafe_code)]
use avm_contract_sdk::{AccountAddress, CurrencyAmounts, MessageInput};
pub mod ids;
pub mod state;
pub type ResultCode<T> = Result<T, u32>;
pub const MAX_STREAMS: usize = 8;
pub const E_ARGS: u32 = 4000;
pub const E_AUTH: u32 = 4001;
pub const E_STATE: u32 = 4002;
pub const E_VALUE: u32 = 4003;
pub const E_PHASE: u32 = 4004;
pub const E_EMPTY: u32 = 4005;
pub const E_BUSY: u32 = 4006;
pub const E_CALLBACK: u32 = 4007;
pub const E_OVERFLOW: u32 = 4008;
pub const E_BALANCE: u32 = 4009;
pub const E_IRREVOCABLE: u32 = 4010;
// This example explicitly chooses ECC0 as its synthetic test principal.
// It does not assign an asset to ECC0 or convert an older AVM currency format.
pub const PRINCIPAL_CURRENCY_ID: u32 = 0;
pub const SHELL_CURRENCY_ID: u32 = 2;

pub fn principal_value(extra: CurrencyAmounts<'_>) -> Option<u128> {
    if extra.len() != 1 {
        return None;
    }
    extra.iter().next().filter(|c| c.currency_id == PRINCIPAL_CURRENCY_ID).map(|c| c.amount)
}

pub fn principal_balance(extra: CurrencyAmounts<'_>) -> u128 {
    extra.iter().find(|c| c.currency_id == PRINCIPAL_CURRENCY_ID).map_or(0, |c| c.amount)
}
pub fn array<const N: usize>(b: &[u8], offset: usize) -> ResultCode<[u8; N]> {
    let end = offset.checked_add(N).ok_or(E_ARGS)?;
    b.get(offset..end).and_then(|v| v.try_into().ok()).ok_or(E_ARGS)
}
pub fn u128_at(b: &[u8], o: usize) -> ResultCode<u128> {
    array(b, o).map(u128::from_le_bytes)
}
pub fn u64_at(b: &[u8], o: usize) -> ResultCode<u64> {
    array(b, o).map(u64::from_le_bytes)
}
pub fn u32_at(b: &[u8], o: usize) -> ResultCode<u32> {
    array(b, o).map(u32::from_le_bytes)
}
pub fn address_at(b: &[u8], o: usize) -> ResultCode<AccountAddress> {
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
pub fn no_body(b: &[u8]) -> ResultCode<()> {
    if b.is_empty() {
        Ok(())
    } else {
        Err(E_ARGS)
    }
}

/// Borrow the constructor data through the SDK's canonical envelope parser.
pub fn deploy_data(input: MessageInput<'_>) -> ResultCode<&[u8]> {
    input.deploy().map(|deploy| deploy.init_data()).map_err(|_| E_ARGS)
}

/// Exact floor(total * elapsed / duration), with no overflowing u128 product.
pub fn vested(
    total: u128,
    start: u32,
    cliff: u32,
    end: u32,
    step: u32,
    now: u32,
) -> ResultCode<u128> {
    if total == 0
        || start == 0
        || start >= end
        || cliff < start
        || cliff > end
        || step > end - start
    {
        return Err(E_ARGS);
    }
    if now < cliff || now <= start {
        return Ok(0);
    }
    if now >= end {
        return Ok(total);
    }
    let duration = u128::from(end - start);
    let elapsed = now - start;
    let elapsed = u128::from(if step == 0 { elapsed } else { (elapsed / step) * step });
    (total / duration)
        .checked_mul(elapsed)
        .and_then(|v| v.checked_add((total % duration) * elapsed / duration))
        .ok_or(E_OVERFLOW)
}
pub fn payment_body(index: u8, kind: u8, nonce: u64, amount: u128) -> Vec<u8> {
    let mut b = vec![index, kind];
    b.extend_from_slice(&nonce.to_le_bytes());
    b.extend_from_slice(&amount.to_le_bytes());
    b
}
pub fn read_payment(b: &[u8]) -> ResultCode<(usize, usize, u64, u128)> {
    if b.len() != 26 || usize::from(b[0]) >= MAX_STREAMS || b[1] > 1 {
        return Err(E_ARGS);
    }
    let nonce = u64_at(b, 2)?;
    let amount = u128_at(b, 10)?;
    if nonce == 0 || amount == 0 {
        return Err(E_ARGS);
    }
    Ok((usize::from(b[0]), usize::from(b[1]), nonce, amount))
}
