#![forbid(unsafe_code)]

use avm_contract_sdk::encode_event_fields;
use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::Caller;
use avm_contract_sdk::Contract;
use avm_contract_sdk::EventField;
use avm_contract_sdk::EventIntent;
use avm_contract_sdk::EventKind;
use avm_contract_sdk::EventValue;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::StateUpdate;

// Fixed supply credited by `init` to the holder wallet named in the init data.
const INITIAL_SUPPLY: u64 = 1_000_000;

// State: supply u64 | holder count u32 | reserved u32 | HOLDER_SLOTS x (wallet 64 | balance u64).
const HOLDER_SLOTS: usize = 8;
const HEADER_BYTES: usize = 16;
const ADDRESS_BYTES: usize = 64;
const SLOT_BYTES: usize = ADDRESS_BYTES + 8;
const STATE_BYTES: usize = HEADER_BYTES + HOLDER_SLOTS * SLOT_BYTES;
const TRANSFER_ARGS_BYTES: usize = ADDRESS_BYTES + 8;
const MINT_SOURCE: AccountAddress = AccountAddress { dapp_id: [0; 32], account_id: [0; 32] };

const TRANSFER_METHOD_ID: u64 = 0x2f7d_5d43_3912_8148;
const TRANSFERRED_EVENT_CLASS: u64 = 0xdf65_8b57_45e0_8f51;

const WALLET_CALLER_REQUIRED: u32 = 1300;
const INVALID_TRANSFER_ARGS: u32 = 1301;
const INVALID_TOKEN_STATE: u32 = 1302;
const INSUFFICIENT_BALANCE: u32 = 1303;
const HOLDER_CAPACITY_EXHAUSTED: u32 = 1304;
const INVALID_EVENT: u32 = 1305;
const INVALID_INITIAL_HOLDER: u32 = 1306;

struct FungibleToken;

impl Contract for FungibleToken {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Some(holder) = input.deploy().ok().and_then(|deploy| address(deploy.init_data()))
        else {
            return MessageOutput::failure(INVALID_INITIAL_HOLDER);
        };
        let ledger = Ledger { supply: INITIAL_SUPPLY, holders: vec![(holder, INITIAL_SUPPLY)] };
        transferred(&ledger, MINT_SOURCE, holder, INITIAL_SUPPLY)
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        // A holder spends through its wallet: the caller address is the holder.
        let Caller::Internal(from) = input.caller else {
            return MessageOutput::failure(WALLET_CALLER_REQUIRED);
        };
        if input.message.method_id() != TRANSFER_METHOD_ID {
            return MessageOutput::failure(INVALID_TRANSFER_ARGS);
        }
        let Some((to, amount)) = transfer_args(input.message.body()) else {
            return MessageOutput::failure(INVALID_TRANSFER_ARGS);
        };
        let Some(mut ledger) = Ledger::decode(input.state) else {
            return MessageOutput::failure(INVALID_TOKEN_STATE);
        };
        if let Err(status_code) = ledger.transfer(from, to, amount) {
            return MessageOutput::failure(status_code);
        }
        transferred(&ledger, from, to, amount)
    }
}

struct Ledger {
    supply: u64,
    holders: Vec<(AccountAddress, u64)>,
}

impl Ledger {
    fn decode(state: &[u8]) -> Option<Self> {
        if state.len() != STATE_BYTES {
            return None;
        }
        let supply = u64::from_le_bytes(state.get(..8)?.try_into().ok()?);
        let count = u32::from_le_bytes(state.get(8..12)?.try_into().ok()?);
        let count = usize::try_from(count).ok().filter(|count| *count <= HOLDER_SLOTS)?;
        let mut holders = Vec::with_capacity(count);
        for slot in state[HEADER_BYTES..].as_chunks::<SLOT_BYTES>().0.iter().take(count) {
            let holder = address(slot.get(..ADDRESS_BYTES)?)?;
            let balance = u64::from_le_bytes(slot.get(ADDRESS_BYTES..)?.try_into().ok()?);
            holders.push((holder, balance));
        }
        let total =
            holders.iter().try_fold(0_u64, |total, (_, balance)| total.checked_add(*balance))?;
        (total == supply).then_some(Self { supply, holders })
    }

    fn encode(&self) -> Vec<u8> {
        let mut state = vec![0; STATE_BYTES];
        state[..8].copy_from_slice(&self.supply.to_le_bytes());
        let count = u32::try_from(self.holders.len()).unwrap_or(u32::MAX);
        state[8..12].copy_from_slice(&count.to_le_bytes());
        let slots = state[HEADER_BYTES..].as_chunks_mut::<SLOT_BYTES>().0.iter_mut();
        for (slot, (holder, balance)) in slots.zip(&self.holders) {
            slot[..ADDRESS_BYTES].copy_from_slice(&address_bytes(*holder));
            slot[ADDRESS_BYTES..].copy_from_slice(&balance.to_le_bytes());
        }
        state
    }

    fn transfer(
        &mut self,
        from: AccountAddress,
        to: AccountAddress,
        amount: u64,
    ) -> Result<(), u32> {
        let sender = self.holders.iter_mut().find(|(holder, _)| *holder == from);
        let Some((_, balance)) = sender else {
            return Err(INSUFFICIENT_BALANCE);
        };
        *balance = balance.checked_sub(amount).ok_or(INSUFFICIENT_BALANCE)?;
        if let Some((_, balance)) = self.holders.iter_mut().find(|(holder, _)| *holder == to) {
            *balance = balance.checked_add(amount).ok_or(INVALID_TOKEN_STATE)?;
            return Ok(());
        }
        if self.holders.len() == HOLDER_SLOTS {
            return Err(HOLDER_CAPACITY_EXHAUSTED);
        }
        self.holders.push((to, amount));
        Ok(())
    }
}

// A holder is a nonzero wallet address: DApp ID then account ID.
fn address(bytes: &[u8]) -> Option<AccountAddress> {
    if bytes.len() != ADDRESS_BYTES {
        return None;
    }
    let holder = AccountAddress {
        dapp_id: bytes.get(..32)?.try_into().ok()?,
        account_id: bytes.get(32..)?.try_into().ok()?,
    };
    (holder != MINT_SOURCE).then_some(holder)
}

fn address_bytes(holder: AccountAddress) -> [u8; ADDRESS_BYTES] {
    let mut bytes = [0; ADDRESS_BYTES];
    bytes[..32].copy_from_slice(&holder.dapp_id);
    bytes[32..].copy_from_slice(&holder.account_id);
    bytes
}

fn transfer_args(body: &[u8]) -> Option<(AccountAddress, u64)> {
    if body.len() != TRANSFER_ARGS_BYTES {
        return None;
    }
    let to = address(body.get(..ADDRESS_BYTES)?)?;
    let amount = u64::from_le_bytes(body.get(ADDRESS_BYTES..)?.try_into().ok()?);
    (amount != 0).then_some((to, amount))
}

fn transferred(
    ledger: &Ledger,
    from: AccountAddress,
    to: AccountAddress,
    amount: u64,
) -> MessageOutput {
    let (from, to) = (address_bytes(from).to_vec(), address_bytes(to).to_vec());
    let Ok(fields_binary) = encode_event_fields(&[
        EventField { name: "amount".into(), value: EventValue::U64(amount) },
        EventField { name: "from".into(), value: EventValue::Data(from.clone()) },
        EventField { name: "to".into(), value: EventValue::Data(to.clone()) },
    ]) else {
        return MessageOutput::failure(INVALID_EVENT);
    };
    let mut output = MessageOutput::success(StateUpdate::FullReplace(ledger.encode()));
    output.event_intents = vec![EventIntent {
        input_index: 0,
        kind: EventKind::External,
        event_class: TRANSFERRED_EVENT_CLASS,
        event_name: "token.transferred".into(),
        topics: vec![from, to],
        fields_binary,
    }];
    output
}

avm_contract_sdk::export_message_contract!(FungibleToken);
