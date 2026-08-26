use crate::{
    AccountAddress, AvmAmount, CurrencyAmount, ExecutionContext, SelectedCodeId, StateOutputMode,
};

const INTERNAL_HEADER_LEN: usize = 192;
const EXTERNAL_HEADER_LEN: usize = 208;
const EXTRA_CURRENCY_LEN: usize = 20;
const INTERNAL_EVENT_READ_LEN: usize = 72;

const INTERNAL_FLAG_CALLBACK: u8 = 1;
const INTERNAL_FLAG_DEPLOY: u8 = 1 << 1;
const INTERNAL_FLAG_CALLBACK_REQUEST: u8 = 1 << 2;
const INTERNAL_FLAG_EVENT_READS: u8 = 1 << 3;
const EXTERNAL_FLAG_SIGNED: u8 = 1;
const EXTERNAL_FLAG_DEPLOY: u8 = 1 << 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    UnexpectedEof,
    IntegerOverflow,
    BadMagic,
    NonZeroReserved,
    UnknownTag,
    InvalidLength,
    InvalidValue,
    Unsorted,
    CallerMismatch,
    DestinationMismatch,
    CallbackForbidden,
    TrailingBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Caller {
    Internal(AccountAddress),
    External([u8; 32]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackHeader {
    pub cause_msg_hash: [u8; 32],
    pub cause_address: AccountAddress,
    pub status_code: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CurrencyAmounts<'a> {
    bytes: &'a [u8],
    count: u16,
}

impl<'a> CurrencyAmounts<'a> {
    pub const fn len(self) -> usize {
        self.count as usize
    }

    pub const fn is_empty(self) -> bool {
        self.count == 0
    }

    pub const fn canonical_bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn iter(self) -> CurrencyAmountIter<'a> {
        CurrencyAmountIter { bytes: self.bytes, position: 0 }
    }
}

impl<'a> IntoIterator for CurrencyAmounts<'a> {
    type Item = CurrencyAmount;
    type IntoIter = CurrencyAmountIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct CurrencyAmountIter<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Iterator for CurrencyAmountIter<'_> {
    type Item = CurrencyAmount;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.position.checked_add(EXTRA_CURRENCY_LEN)?;
        let bytes = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(CurrencyAmount {
            currency_id: u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?),
            amount: u128::from_le_bytes(bytes.get(4..20)?.try_into().ok()?),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.bytes.len().saturating_sub(self.position) / EXTRA_CURRENCY_LEN;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CurrencyAmountIter<'_> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalEventReadRequests<'a> {
    canonical_bytes: &'a [u8],
    count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalEventReadRequest {
    pub source: AccountAddress,
    pub event_class: u64,
}

#[derive(Clone, Debug)]
pub struct InternalEventReadRequestIter<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> InternalEventReadRequests<'a> {
    pub const fn len(self) -> usize {
        self.count as usize
    }

    pub const fn is_empty(self) -> bool {
        self.count == 0
    }

    pub const fn canonical_bytes(self) -> &'a [u8] {
        self.canonical_bytes
    }

    pub fn iter(self) -> InternalEventReadRequestIter<'a> {
        InternalEventReadRequestIter {
            bytes: self.canonical_bytes.get(4..).unwrap_or(&[]),
            position: 0,
        }
    }
}

impl<'a> IntoIterator for InternalEventReadRequests<'a> {
    type Item = InternalEventReadRequest;
    type IntoIter = InternalEventReadRequestIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Iterator for InternalEventReadRequestIter<'_> {
    type Item = InternalEventReadRequest;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.position.checked_add(INTERNAL_EVENT_READ_LEN)?;
        let bytes = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(InternalEventReadRequest {
            source: AccountAddress {
                dapp_id: bytes.get(..32)?.try_into().ok()?,
                account_id: bytes.get(32..64)?.try_into().ok()?,
            },
            event_class: u64::from_le_bytes(bytes.get(64..72)?.try_into().ok()?),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.bytes.len().saturating_sub(self.position) / INTERNAL_EVENT_READ_LEN;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for InternalEventReadRequestIter<'_> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalMessage<'a> {
    pub flags: u8,
    pub method_id: u64,
    pub value_main: AvmAmount,
    pub fuel_fee_shells: AvmAmount,
    pub created_lt: u64,
    pub created_at: u32,
    pub source: AccountAddress,
    pub destination: AccountAddress,
    pub callback: Option<CallbackHeader>,
    pub callback_handler_method_id: Option<u64>,
    pub event_reads: Option<InternalEventReadRequests<'a>>,
    pub extra: CurrencyAmounts<'a>,
    pub body: &'a [u8],
    pub canonical_bytes: &'a [u8],
}

impl InternalMessage<'_> {
    pub const fn is_callback(self) -> bool {
        self.flags & INTERNAL_FLAG_CALLBACK != 0
    }

    pub const fn is_deploy(self) -> bool {
        self.flags & INTERNAL_FLAG_DEPLOY != 0
    }

    pub const fn has_callback_request(self) -> bool {
        self.flags & INTERNAL_FLAG_CALLBACK_REQUEST != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalMessage<'a> {
    pub flags: u8,
    pub method_id: u64,
    pub value_main: AvmAmount,
    pub fuel_fee_shells: AvmAmount,
    pub nonce: u64,
    pub created_at: u32,
    pub expire_at: u32,
    pub destination: AccountAddress,
    pub pubkey: [u8; 32],
    pub extra: CurrencyAmounts<'a>,
    pub body: &'a [u8],
    pub signature: &'a [u8],
    pub canonical_bytes: &'a [u8],
}

impl ExternalMessage<'_> {
    pub const fn is_deploy(self) -> bool {
        self.flags & EXTERNAL_FLAG_DEPLOY != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalMessage<'a> {
    Internal(InternalMessage<'a>),
    External(ExternalMessage<'a>),
}

impl<'a> CanonicalMessage<'a> {
    pub const fn method_id(self) -> u64 {
        match self {
            Self::Internal(message) => message.method_id,
            Self::External(message) => message.method_id,
        }
    }

    pub const fn body(self) -> &'a [u8] {
        match self {
            Self::Internal(message) => message.body,
            Self::External(message) => message.body,
        }
    }

    pub const fn destination(self) -> AccountAddress {
        match self {
            Self::Internal(message) => message.destination,
            Self::External(message) => message.destination,
        }
    }

    pub const fn value_main(self) -> AvmAmount {
        match self {
            Self::Internal(message) => message.value_main,
            Self::External(message) => message.value_main,
        }
    }

    pub const fn fuel_fee_shells(self) -> AvmAmount {
        match self {
            Self::Internal(message) => message.fuel_fee_shells,
            Self::External(message) => message.fuel_fee_shells,
        }
    }

    pub const fn extra(self) -> CurrencyAmounts<'a> {
        match self {
            Self::Internal(message) => message.extra,
            Self::External(message) => message.extra,
        }
    }

    pub const fn is_deploy(self) -> bool {
        match self {
            Self::Internal(message) => message.is_deploy(),
            Self::External(message) => message.is_deploy(),
        }
    }

    pub const fn has_callback_request(self) -> bool {
        matches!(self, Self::Internal(message) if message.has_callback_request())
    }

    pub const fn is_callback(self) -> bool {
        matches!(self, Self::Internal(message) if message.is_callback())
    }

    pub const fn canonical_bytes(self) -> &'a [u8] {
        match self {
            Self::Internal(message) => message.canonical_bytes,
            Self::External(message) => message.canonical_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageInput<'a> {
    pub context: ExecutionContext,
    pub caller: Caller,
    pub destination: AccountAddress,
    pub selected_code_id: SelectedCodeId,
    pub required_state_output_mode: StateOutputMode,
    pub balance_main: AvmAmount,
    pub extra: CurrencyAmounts<'a>,
    pub state: &'a [u8],
    pub message: CanonicalMessage<'a>,
    pub internal_event_results: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchInput<'a> {
    pub context: ExecutionContext,
    pub destination: AccountAddress,
    pub selected_code_id: SelectedCodeId,
    pub required_state_output_mode: StateOutputMode,
    pub balance_main: AvmAmount,
    pub extra: CurrencyAmounts<'a>,
    pub state: &'a [u8],
    offsets: &'a [u8],
    messages: &'a [u8],
    message_count: u32,
    pub internal_event_results: &'a [u8],
}

impl<'a> BatchInput<'a> {
    pub const fn message_count(self) -> u32 {
        self.message_count
    }

    pub const fn messages(self) -> BatchMessageIter<'a> {
        BatchMessageIter {
            offsets: self.offsets,
            messages: self.messages,
            index: 0,
            count: self.message_count,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BatchMessageIter<'a> {
    offsets: &'a [u8],
    messages: &'a [u8],
    index: u32,
    count: u32,
}

impl<'a> Iterator for BatchMessageIter<'a> {
    type Item = Result<CanonicalMessage<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            return None;
        }
        let start = read_offset(self.offsets, self.index);
        let end = read_offset(self.offsets, self.index.saturating_add(1));
        self.index = self.index.saturating_add(1);
        Some(match (start, end) {
            (Ok(start), Ok(end)) => self
                .messages
                .get(start..end)
                .ok_or(DecodeError::InvalidLength)
                .and_then(decode_canonical_message),
            (Err(error), _) | (_, Err(error)) => Err(error),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count.saturating_sub(self.index) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for BatchMessageIter<'_> {}

pub fn decode_message_input(bytes: &[u8]) -> Result<MessageInput<'_>, DecodeError> {
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != b"ACMI" {
        return Err(DecodeError::BadMagic);
    }
    let context = read_context(&mut reader)?;
    let caller_kind = reader.u8()?;
    require_zero(reader.take(7)?)?;
    let src_dapp_id = reader.array()?;
    let src_account_id = reader.array()?;
    let external_pubkey = reader.array()?;
    let destination = AccountAddress { dapp_id: reader.array()?, account_id: reader.array()? };
    let selected_code_id = reader.array()?;
    let required_state_output_mode = read_state_output_mode(reader.u8()?)?;
    require_zero(reader.take(7)?)?;
    let balance_main = reader.u128()?;
    let extra = read_currency_amounts(&mut reader)?;
    let state = reader.length_prefixed_u32()?;
    let message_bytes = reader.length_prefixed_u32()?;
    let internal_event_results = reader.length_prefixed_u32()?;
    reader.finish()?;

    let message = decode_canonical_message(message_bytes)?;
    if message.destination() != destination {
        return Err(DecodeError::DestinationMismatch);
    }
    let caller = match (caller_kind, message) {
        (0, CanonicalMessage::Internal(internal))
            if external_pubkey == [0; 32]
                && src_dapp_id == internal.source.dapp_id
                && src_account_id == internal.source.account_id =>
        {
            Caller::Internal(internal.source)
        }
        (1, CanonicalMessage::External(external))
            if src_dapp_id == [0; 32]
                && src_account_id == [0; 32]
                && external_pubkey == external.pubkey =>
        {
            Caller::External(external_pubkey)
        }
        (0 | 1, _) => return Err(DecodeError::CallerMismatch),
        _ => return Err(DecodeError::UnknownTag),
    };

    Ok(MessageInput {
        context,
        caller,
        destination,
        selected_code_id,
        required_state_output_mode,
        balance_main,
        extra,
        state,
        message,
        internal_event_results,
    })
}

pub fn decode_batch_input(bytes: &[u8]) -> Result<BatchInput<'_>, DecodeError> {
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != b"ACBI" {
        return Err(DecodeError::BadMagic);
    }
    let context = read_context(&mut reader)?;
    let destination = AccountAddress { dapp_id: reader.array()?, account_id: reader.array()? };
    let selected_code_id = reader.array()?;
    let required_state_output_mode = read_state_output_mode(reader.u8()?)?;
    require_zero(reader.take(7)?)?;
    let balance_main = reader.u128()?;
    let extra = read_currency_amounts(&mut reader)?;
    let state = reader.length_prefixed_u32()?;
    let message_count = reader.u32()?;
    let offset_count = message_count.checked_add(1).ok_or(DecodeError::IntegerOverflow)?;
    let offsets_len = usize::try_from(offset_count)
        .map_err(|_| DecodeError::IntegerOverflow)?
        .checked_mul(4)
        .ok_or(DecodeError::IntegerOverflow)?;
    let offsets = reader.take(offsets_len)?;
    if read_offset(offsets, 0)? != 0 {
        return Err(DecodeError::InvalidLength);
    }
    let messages_len = read_offset(offsets, message_count)?;
    let messages = reader.take(messages_len)?;
    let internal_event_results = reader.length_prefixed_u32()?;
    reader.finish()?;

    let mut previous = 0usize;
    for index in 0..message_count {
        let end = read_offset(offsets, index + 1)?;
        if end <= previous || end > messages.len() {
            return Err(DecodeError::InvalidLength);
        }
        let message = decode_canonical_message(
            messages.get(previous..end).ok_or(DecodeError::InvalidLength)?,
        )?;
        if message.destination() != destination {
            return Err(DecodeError::DestinationMismatch);
        }
        if message.is_callback() || message.has_callback_request() {
            return Err(DecodeError::CallbackForbidden);
        }
        previous = end;
    }

    Ok(BatchInput {
        context,
        destination,
        selected_code_id,
        required_state_output_mode,
        balance_main,
        extra,
        state,
        offsets,
        messages,
        message_count,
        internal_event_results,
    })
}

fn decode_canonical_message(bytes: &[u8]) -> Result<CanonicalMessage<'_>, DecodeError> {
    match bytes.first().copied().ok_or(DecodeError::UnexpectedEof)? {
        0 => decode_internal_message(bytes).map(CanonicalMessage::Internal),
        1 => decode_external_message(bytes).map(CanonicalMessage::External),
        _ => Err(DecodeError::UnknownTag),
    }
}

fn decode_internal_message(bytes: &[u8]) -> Result<InternalMessage<'_>, DecodeError> {
    let header = bytes.get(..INTERNAL_HEADER_LEN).ok_or(DecodeError::UnexpectedEof)?;
    let flags = header[1];
    if flags
        & !(INTERNAL_FLAG_CALLBACK
            | INTERNAL_FLAG_DEPLOY
            | INTERNAL_FLAG_CALLBACK_REQUEST
            | INTERNAL_FLAG_EVENT_READS)
        != 0
        || flags & INTERNAL_FLAG_CALLBACK != 0
            && flags & (INTERNAL_FLAG_DEPLOY | INTERNAL_FLAG_CALLBACK_REQUEST) != 0
    {
        return Err(DecodeError::InvalidValue);
    }
    require_zero(header.get(60..64).ok_or(DecodeError::UnexpectedEof)?)?;
    let extra_count = read_u16_at(header, 2)?;
    let body_len = to_usize(read_u32_at(header, 4)?)?;
    let source = AccountAddress {
        dapp_id: read_array_at(header, 64)?,
        account_id: read_array_at(header, 96)?,
    };
    let destination = AccountAddress {
        dapp_id: read_array_at(header, 128)?,
        account_id: read_array_at(header, 160)?,
    };
    if is_zero_address(source) || is_zero_address(destination) {
        return Err(DecodeError::InvalidValue);
    }

    let mut reader = Reader::with_position(bytes, INTERNAL_HEADER_LEN)?;
    let callback = if flags & INTERNAL_FLAG_CALLBACK != 0 {
        let cause_msg_hash = reader.array()?;
        let cause_address =
            AccountAddress { dapp_id: reader.array()?, account_id: reader.array()? };
        let status_code = reader.u32()?;
        if cause_msg_hash == [0; 32]
            || is_zero_address(cause_address)
            || cause_address != destination
        {
            return Err(DecodeError::InvalidValue);
        }
        Some(CallbackHeader { cause_msg_hash, cause_address, status_code })
    } else {
        None
    };
    let callback_handler_method_id = if flags & INTERNAL_FLAG_CALLBACK_REQUEST != 0 {
        let method_id = reader.u64()?;
        if method_id == 0 {
            return Err(DecodeError::InvalidValue);
        }
        Some(method_id)
    } else {
        None
    };
    let event_reads = if flags & INTERNAL_FLAG_EVENT_READS != 0 {
        let start = reader.position;
        let count = reader.u16()?;
        if count == 0 {
            return Err(DecodeError::InvalidValue);
        }
        require_zero(reader.take(2)?)?;
        let records_len = usize::from(count)
            .checked_mul(INTERNAL_EVENT_READ_LEN)
            .ok_or(DecodeError::IntegerOverflow)?;
        reader.take(records_len)?;
        let requests = InternalEventReadRequests {
            canonical_bytes: bytes.get(start..reader.position).ok_or(DecodeError::InvalidLength)?,
            count,
        };
        let mut previous = None;
        for request in requests {
            let key = (request.source.dapp_id, request.source.account_id, request.event_class);
            if previous.is_some_and(|previous| previous >= key) {
                return Err(DecodeError::Unsorted);
            }
            previous = Some(key);
        }
        Some(requests)
    } else {
        None
    };
    let extra = read_currency_amounts_with_count(&mut reader, extra_count)?;
    let body = reader.take(body_len)?;
    reader.finish()?;

    if let Some(callback) = callback {
        if read_u64_at(header, 8)? == 0 && callback.status_code == 0
            || callback.status_code != 0 && !body.is_empty()
        {
            return Err(DecodeError::InvalidValue);
        }
    }

    Ok(InternalMessage {
        flags,
        method_id: read_u64_at(header, 8)?,
        value_main: read_u128_at(header, 16)?,
        fuel_fee_shells: read_u128_at(header, 32)?,
        created_lt: read_u64_at(header, 48)?,
        created_at: read_u32_at(header, 56)?,
        source,
        destination,
        callback,
        callback_handler_method_id,
        event_reads,
        extra,
        body,
        canonical_bytes: bytes,
    })
}

fn decode_external_message(bytes: &[u8]) -> Result<ExternalMessage<'_>, DecodeError> {
    let header = bytes.get(..EXTERNAL_HEADER_LEN).ok_or(DecodeError::UnexpectedEof)?;
    let flags = header[1];
    if flags & !(EXTERNAL_FLAG_SIGNED | EXTERNAL_FLAG_DEPLOY) != 0
        || flags & EXTERNAL_FLAG_SIGNED == 0
    {
        return Err(DecodeError::InvalidValue);
    }
    require_zero(header.get(162..208).ok_or(DecodeError::UnexpectedEof)?)?;
    let signature_len = read_u16_at(header, 160)?;
    if signature_len != 64 {
        return Err(DecodeError::InvalidLength);
    }
    let extra_count = read_u16_at(header, 2)?;
    let body_len = to_usize(read_u32_at(header, 4)?)?;
    let destination = AccountAddress {
        dapp_id: read_array_at(header, 64)?,
        account_id: read_array_at(header, 96)?,
    };
    let pubkey = read_array_at(header, 128)?;
    if is_zero_address(destination) || pubkey == [0; 32] {
        return Err(DecodeError::InvalidValue);
    }

    let mut reader = Reader::with_position(bytes, EXTERNAL_HEADER_LEN)?;
    let extra = read_currency_amounts_with_count(&mut reader, extra_count)?;
    let body = reader.take(body_len)?;
    let signature = reader.take(usize::from(signature_len))?;
    reader.finish()?;

    Ok(ExternalMessage {
        flags,
        method_id: read_u64_at(header, 8)?,
        value_main: read_u128_at(header, 16)?,
        fuel_fee_shells: read_u128_at(header, 32)?,
        nonce: read_u64_at(header, 48)?,
        created_at: read_u32_at(header, 56)?,
        expire_at: read_u32_at(header, 60)?,
        destination,
        pubkey,
        extra,
        body,
        signature,
        canonical_bytes: bytes,
    })
}

fn read_context(reader: &mut Reader<'_>) -> Result<ExecutionContext, DecodeError> {
    Ok(ExecutionContext {
        block_lt: reader.u64()?,
        block_time: reader.u32()?,
        block_seq_no: reader.u32()?,
        block_rand: reader.array()?,
    })
}

fn read_currency_amounts<'a>(reader: &mut Reader<'a>) -> Result<CurrencyAmounts<'a>, DecodeError> {
    let count = reader.u16()?;
    read_currency_amounts_with_count(reader, count)
}

fn read_currency_amounts_with_count<'a>(
    reader: &mut Reader<'a>,
    count: u16,
) -> Result<CurrencyAmounts<'a>, DecodeError> {
    let length =
        usize::from(count).checked_mul(EXTRA_CURRENCY_LEN).ok_or(DecodeError::IntegerOverflow)?;
    let bytes = reader.take(length)?;
    let amounts = CurrencyAmounts { bytes, count };
    let mut previous = None;
    for amount in amounts {
        if previous.is_some_and(|currency_id| currency_id >= amount.currency_id) {
            return Err(DecodeError::Unsorted);
        }
        previous = Some(amount.currency_id);
    }
    Ok(amounts)
}

fn read_state_output_mode(value: u8) -> Result<StateOutputMode, DecodeError> {
    match value {
        0 => Ok(StateOutputMode::FullReplace),
        1 => Ok(StateOutputMode::DataPatchList),
        _ => Err(DecodeError::InvalidValue),
    }
}

fn read_offset(bytes: &[u8], index: u32) -> Result<usize, DecodeError> {
    let offset = usize::try_from(index)
        .map_err(|_| DecodeError::IntegerOverflow)?
        .checked_mul(4)
        .ok_or(DecodeError::IntegerOverflow)?;
    to_usize(read_u32_at(bytes, offset)?)
}

fn require_zero(bytes: &[u8]) -> Result<(), DecodeError> {
    if bytes.iter().any(|byte| *byte != 0) {
        Err(DecodeError::NonZeroReserved)
    } else {
        Ok(())
    }
}

fn is_zero_address(address: AccountAddress) -> bool {
    address.dapp_id == [0; 32] || address.account_id == [0; 32]
}

fn to_usize(value: u32) -> Result<usize, DecodeError> {
    usize::try_from(value).map_err(|_| DecodeError::IntegerOverflow)
}

fn read_u16_at(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(read_fixed_at(bytes, offset)?))
}

fn read_u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(read_fixed_at(bytes, offset)?))
}

fn read_u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(read_fixed_at(bytes, offset)?))
}

fn read_u128_at(bytes: &[u8], offset: usize) -> Result<u128, DecodeError> {
    Ok(u128::from_le_bytes(read_fixed_at(bytes, offset)?))
}

fn read_array_at<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], DecodeError> {
    read_fixed_at(bytes, offset)
}

fn read_fixed_at<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], DecodeError> {
    let end = offset.checked_add(N).ok_or(DecodeError::IntegerOverflow)?;
    bytes
        .get(offset..end)
        .ok_or(DecodeError::UnexpectedEof)?
        .try_into()
        .map_err(|_| DecodeError::UnexpectedEof)
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn with_position(bytes: &'a [u8], position: usize) -> Result<Self, DecodeError> {
        if position > bytes.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        Ok(Self { bytes, position })
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.position.checked_add(length).ok_or(DecodeError::IntegerOverflow)?;
        let value = self.bytes.get(self.position..end).ok_or(DecodeError::UnexpectedEof)?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.fixed()?))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn u128(&mut self) -> Result<u128, DecodeError> {
        Ok(u128::from_le_bytes(self.fixed()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        self.fixed()
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        self.take(N)?.try_into().map_err(|_| DecodeError::UnexpectedEof)
    }

    fn length_prefixed_u32(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = to_usize(self.u32()?)?;
        self.take(length)
    }

    fn finish(self) -> Result<(), DecodeError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
}
