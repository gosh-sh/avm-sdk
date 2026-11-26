#[cfg(feature = "event-rings")]
use crate::InternalEventReadRequest;
use alloc::{string::String, vec::Vec};

use crate::{
    AccountAddress, AvmAmount, BatchInput, CodeHash, CurrencyAmount, MessageInput, SelectedCodeId,
    StateOutputMode, CODE_UPGRADE_METHOD_ID,
};

const FLAG_HAS_STATE_UPDATE: u8 = 1;
const FLAG_HAS_UPGRADE_INTENT: u8 = 1 << 1;
#[cfg(feature = "event-rings")]
const FLAG_HAS_OUTBOUND_EVENT_READS: u8 = 1 << 3;
/// `BatchOutput.flags` bit 0: the batch output declares outbound event reads.
#[cfg(feature = "event-rings")]
const BATCH_FLAG_HAS_OUTBOUND_EVENT_READS: u8 = 1;
#[cfg(feature = "system-mint")]
const FLAG_HAS_MINT_INTENT: u8 = 1 << 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    LengthOverflow,
    InvalidStatusCode,
    MissingStateUpdate,
    EffectsOnFailure,
    StateModeMismatch,
    InvalidExtraCurrencies,
    InvalidPatchOrder,
    InvalidIndex,
    InvalidCallback,
    InvalidEvent,
    UpgradeForbidden,
    UpgradeRequired,
    InvalidEventReads,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePatch {
    pub offset: u32,
    pub old_len: u32,
    pub new_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateUpdate {
    FullReplace(Vec<u8>),
    DataPatchList(Vec<StatePatch>),
}

impl StateUpdate {
    pub const fn mode(&self) -> StateOutputMode {
        match self {
            Self::FullReplace(_) => StateOutputMode::FullReplace,
            Self::DataPatchList(_) => StateOutputMode::DataPatchList,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundIntent {
    pub input_index: u32,
    pub destination: AccountAddress,
    pub fuel_fee_shells: AvmAmount,
    pub extra: Vec<CurrencyAmount>,
    pub method_id: u64,
    pub body: Vec<u8>,
}

/// The exact internal-event reads that the internal message of the outbound
/// intent `outbound_index` declares.
/// `reads` is not empty and strictly ascending by source DApp id, source
/// account id and `event_class`; the receiver gets one result for each read,
/// in this order.
#[cfg(feature = "event-rings")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventReadEntry {
    pub outbound_index: u32,
    pub reads: Vec<InternalEventReadRequest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackIntent {
    pub outbound_index: u32,
    pub handler_method_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EventKind {
    External = 0,
    /// Enters the internal-event ring of its emitter; only with the feature
    /// `event-rings`.
    #[cfg(feature = "event-rings")]
    Internal = 1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventIntent {
    pub input_index: u32,
    pub kind: EventKind,
    pub event_class: u64,
    pub event_name: String,
    pub topics: Vec<Vec<u8>>,
    pub fields_binary: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeUpgradeIntent {
    pub target_selected_code_id: SelectedCodeId,
    pub expected_code_hash: CodeHash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageOutput {
    pub status_code: u32,
    pub state_update: Option<StateUpdate>,
    pub callback_result_body: Vec<u8>,
    pub outbound_intents: Vec<OutboundIntent>,
    pub callback_intents: Vec<CallbackIntent>,
    pub event_intents: Vec<EventIntent>,
    pub code_upgrade_intent: Option<CodeUpgradeIntent>,
    /// Entries in strictly ascending `outbound_index` order; only with the
    /// feature `event-rings`.
    #[cfg(feature = "event-rings")]
    pub outbound_event_reads: Vec<EventReadEntry>,
}

impl MessageOutput {
    pub fn success(state_update: StateUpdate) -> Self {
        Self {
            status_code: 0,
            state_update: Some(state_update),
            callback_result_body: Vec::new(),
            outbound_intents: Vec::new(),
            callback_intents: Vec::new(),
            event_intents: Vec::new(),
            code_upgrade_intent: None,
            #[cfg(feature = "event-rings")]
            outbound_event_reads: Vec::new(),
        }
    }

    pub fn failure(status_code: u32) -> Self {
        Self {
            status_code,
            state_update: None,
            callback_result_body: Vec::new(),
            outbound_intents: Vec::new(),
            callback_intents: Vec::new(),
            event_intents: Vec::new(),
            code_upgrade_intent: None,
            #[cfg(feature = "event-rings")]
            outbound_event_reads: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchOutput {
    pub state_update: StateUpdate,
    pub status_codes: Vec<u32>,
    pub outbound_intents: Vec<OutboundIntent>,
    pub event_intents: Vec<EventIntent>,
    /// Entries in strictly ascending `outbound_index` order; only with the
    /// feature `event-rings`.
    #[cfg(feature = "event-rings")]
    pub outbound_event_reads: Vec<EventReadEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventField {
    pub name: String,
    pub value: EventValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventValue {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    U128(u128),
    I128(i128),
    Text(String),
    Data(Vec<u8>),
    Address(AccountAddress),
}

pub fn encode_event_fields(fields: &[EventField]) -> Result<Vec<u8>, EncodeError> {
    let field_count = u16::try_from(fields.len()).map_err(|_| EncodeError::LengthOverflow)?;
    let mut bytes = Vec::new();
    push_u16(&mut bytes, field_count);
    push_u16(&mut bytes, 0);
    let mut previous_name: Option<&[u8]> = None;
    for field in fields {
        let name = field.name.as_bytes();
        if previous_name.is_some_and(|previous| previous >= name) {
            return Err(EncodeError::InvalidEvent);
        }
        previous_name = Some(name);
        push_u16(&mut bytes, len_u16(name.len())?);
        let (kind, value) = encode_event_value(&field.value)?;
        bytes.push(kind);
        push_u16(&mut bytes, len_u16(value.len())?);
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&value);
    }
    Ok(bytes)
}

pub fn encode_message_output(output: &MessageOutput) -> Result<Vec<u8>, EncodeError> {
    encode_message_output_inner(output, None)
}

pub fn encode_batch_output(output: &BatchOutput) -> Result<Vec<u8>, EncodeError> {
    encode_batch_output_inner(output, None)
}

// Used by the WASM entrypoint dispatch and by the unit tests.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn message_output_len_for(
    input: &MessageInput<'_>,
    is_init: bool,
    output: &MessageOutput,
) -> Result<usize, EncodeError> {
    validate_message_context(input, is_init, output)?;
    validate_message_shape(output, Some(input.required_state_output_mode))?;
    message_output_len(output)
}

#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn batch_output_len_for(
    input: &BatchInput<'_>,
    output: &BatchOutput,
) -> Result<usize, EncodeError> {
    validate_batch_context(input, output)?;
    validate_batch_shape(output, Some(input.required_state_output_mode))?;
    batch_output_len(output)
}

pub(crate) trait OutputWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError>;
}

impl OutputWriter for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

fn encode_message_output_inner(
    output: &MessageOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    validate_message_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    write_message_output(&mut bytes, output)?;
    Ok(bytes)
}

pub(crate) fn write_message_output(
    bytes: &mut impl OutputWriter,
    output: &MessageOutput,
) -> Result<(), EncodeError> {
    bytes.write(b"ACMO")?;
    bytes.write(&output.status_code.to_le_bytes())?;

    let mut flags = 0;
    if output.state_update.is_some() {
        flags |= FLAG_HAS_STATE_UPDATE;
    }
    if output.code_upgrade_intent.is_some() {
        flags |= FLAG_HAS_UPGRADE_INTENT;
    }
    #[cfg(feature = "event-rings")]
    if !output.outbound_event_reads.is_empty() {
        flags |= FLAG_HAS_OUTBOUND_EVENT_READS;
    }
    bytes.write(&[flags])?;
    if let Some(state_update) = &output.state_update {
        encode_state_update(bytes, state_update, 0)?;
    }
    push_length_prefixed_u32(bytes, &output.callback_result_body)?;
    encode_outbound_intents(bytes, &output.outbound_intents)?;
    bytes.write(&len_u32(output.callback_intents.len())?.to_le_bytes())?;
    for callback in &output.callback_intents {
        bytes.write(&callback.outbound_index.to_le_bytes())?;
        bytes.write(&callback.handler_method_id.to_le_bytes())?;
    }
    encode_event_intents(bytes, &output.event_intents)?;
    if let Some(upgrade) = &output.code_upgrade_intent {
        bytes.write(&upgrade.target_selected_code_id)?;
        bytes.write(&upgrade.expected_code_hash)?;
    }
    #[cfg(feature = "event-rings")]
    encode_event_reads(bytes, &output.outbound_event_reads)?;
    Ok(())
}

/// Encodes the output of a system minter: the
/// ordinary encoding with the `has_mint_intent` flag, then `mint_count` and
/// `mints` in the `MessageOutput` layout. Empty `mints` encode the
/// ordinary output. The host checks the count limit and the designation.
#[cfg(feature = "system-mint")]
pub fn encode_mint_output(
    output: &MessageOutput,
    mints: &[CurrencyAmount],
) -> Result<Vec<u8>, EncodeError> {
    validate_message_shape(output, None)?;
    validate_mints(output, mints)?;
    let mut bytes = Vec::new();
    write_mint_output(&mut bytes, output, mints)?;
    Ok(bytes)
}

#[cfg(feature = "system-mint")]
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn mint_output_len_for(
    input: &MessageInput<'_>,
    output: &MessageOutput,
    mints: &[CurrencyAmount],
) -> Result<usize, EncodeError> {
    let len = message_output_len_for(input, false, output)?;
    if mints.is_empty() {
        return Ok(len);
    }
    validate_mints(output, mints)?;
    checked_len(&[len, 2, repeated_len(mints.len(), 20)?])
}

/// A mint intent belongs to a successful output; its amounts are non-zero
/// and its currency ids strictly ascending.
#[cfg(feature = "system-mint")]
fn validate_mints(output: &MessageOutput, mints: &[CurrencyAmount]) -> Result<(), EncodeError> {
    if mints.is_empty() {
        return Ok(());
    }
    if output.status_code != 0 {
        return Err(EncodeError::EffectsOnFailure);
    }
    len_u16(mints.len())?;
    if mints.iter().any(|mint| mint.amount == 0)
        || mints.windows(2).any(|pair| pair[0].currency_id >= pair[1].currency_id)
    {
        return Err(EncodeError::InvalidExtraCurrencies);
    }
    Ok(())
}

#[cfg(feature = "system-mint")]
pub(crate) fn write_mint_output(
    bytes: &mut impl OutputWriter,
    output: &MessageOutput,
    mints: &[CurrencyAmount],
) -> Result<(), EncodeError> {
    if mints.is_empty() {
        return write_message_output(bytes, output);
    }
    // The flags byte follows the 4-byte magic and the 4-byte status code.
    let mut encoded = Vec::new();
    write_message_output(&mut encoded, output)?;
    encoded[8] |= FLAG_HAS_MINT_INTENT;
    // The mint intent precedes the trailing `OutboundEventReads`.
    #[cfg(feature = "event-rings")]
    let reads_at = encoded.len() - event_reads_len(&output.outbound_event_reads)?;
    #[cfg(not(feature = "event-rings"))]
    let reads_at = encoded.len();
    bytes.write(&encoded[..reads_at])?;
    bytes.write(&len_u16(mints.len())?.to_le_bytes())?;
    for mint in mints {
        bytes.write(&mint.currency_id.to_le_bytes())?;
        bytes.write(&mint.amount.to_le_bytes())?;
    }
    #[cfg(feature = "event-rings")]
    bytes.write(&encoded[reads_at..])?;
    Ok(())
}

fn encode_batch_output_inner(
    output: &BatchOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    validate_batch_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    write_batch_output(&mut bytes, output)?;
    Ok(bytes)
}

pub(crate) fn write_batch_output(
    bytes: &mut impl OutputWriter,
    output: &BatchOutput,
) -> Result<(), EncodeError> {
    bytes.write(b"ACBO")?;
    #[cfg(feature = "event-rings")]
    let flags = if output.outbound_event_reads.is_empty() {
        0
    } else {
        BATCH_FLAG_HAS_OUTBOUND_EVENT_READS
    };
    #[cfg(not(feature = "event-rings"))]
    let flags = 0;
    encode_state_update(bytes, &output.state_update, flags)?;
    bytes.write(&len_u32(output.status_codes.len())?.to_le_bytes())?;
    for status_code in &output.status_codes {
        bytes.write(&status_code.to_le_bytes())?;
    }
    encode_outbound_intents(bytes, &output.outbound_intents)?;
    encode_event_intents(bytes, &output.event_intents)?;
    #[cfg(feature = "event-rings")]
    encode_event_reads(bytes, &output.outbound_event_reads)?;
    Ok(())
}

// Size only validated lengths/counts; never serialize or scan payload bytes twice.
fn checked_len(parts: &[usize]) -> Result<usize, EncodeError> {
    parts
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(*part).ok_or(EncodeError::LengthOverflow))
}

fn repeated_len(count: usize, width: usize) -> Result<usize, EncodeError> {
    count.checked_mul(width).ok_or(EncodeError::LengthOverflow)
}

fn state_update_len(update: &StateUpdate) -> Result<usize, EncodeError> {
    match update {
        StateUpdate::FullReplace(state) => checked_len(&[8, state.len()]),
        StateUpdate::DataPatchList(patches) => patches
            .iter()
            .try_fold(8, |total, patch| checked_len(&[total, 12, patch.new_bytes.len()])),
    }
}

fn outbound_intents_len(intents: &[OutboundIntent]) -> Result<usize, EncodeError> {
    intents.iter().try_fold(4, |total, intent| {
        checked_len(&[total, 98, repeated_len(intent.extra.len(), 20)?, intent.body.len()])
    })
}

fn event_intents_len(intents: &[EventIntent]) -> Result<usize, EncodeError> {
    intents.iter().try_fold(4, |total, intent| {
        let topics = intent
            .topics
            .iter()
            .try_fold(0, |total, topic| checked_len(&[total, 2, topic.len()]))?;
        checked_len(&[total, 21, intent.event_name.len(), topics, intent.fields_binary.len()])
    })
}

#[cfg(feature = "event-rings")]
fn event_reads_len(entries: &[EventReadEntry]) -> Result<usize, EncodeError> {
    if entries.is_empty() {
        return Ok(0);
    }
    entries
        .iter()
        .try_fold(4, |total, entry| checked_len(&[total, 8, repeated_len(entry.reads.len(), 72)?]))
}

fn message_output_len(output: &MessageOutput) -> Result<usize, EncodeError> {
    checked_len(&[
        9,
        output.state_update.as_ref().map(state_update_len).transpose()?.unwrap_or(0),
        4,
        output.callback_result_body.len(),
        outbound_intents_len(&output.outbound_intents)?,
        4,
        repeated_len(output.callback_intents.len(), 12)?,
        event_intents_len(&output.event_intents)?,
        if output.code_upgrade_intent.is_some() { 64 } else { 0 },
        #[cfg(feature = "event-rings")]
        event_reads_len(&output.outbound_event_reads)?,
    ])
}

fn batch_output_len(output: &BatchOutput) -> Result<usize, EncodeError> {
    checked_len(&[
        4,
        state_update_len(&output.state_update)?,
        4,
        repeated_len(output.status_codes.len(), 4)?,
        outbound_intents_len(&output.outbound_intents)?,
        event_intents_len(&output.event_intents)?,
        #[cfg(feature = "event-rings")]
        event_reads_len(&output.outbound_event_reads)?,
    ])
}

fn validate_message_shape(
    output: &MessageOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<(), EncodeError> {
    validate_status_code(output.status_code)?;
    if output.status_code == 0 {
        let update = output.state_update.as_ref().ok_or(EncodeError::MissingStateUpdate)?;
        if required_mode.is_some_and(|mode| mode != update.mode()) {
            return Err(EncodeError::StateModeMismatch);
        }
    } else if output.state_update.is_some()
        || !output.callback_result_body.is_empty()
        || !output.outbound_intents.is_empty()
        || !output.callback_intents.is_empty()
        || !output.event_intents.is_empty()
        || output.code_upgrade_intent.is_some()
        || has_event_reads(output)
    {
        return Err(EncodeError::EffectsOnFailure);
    }

    validate_state_update(output.state_update.as_ref())?;
    validate_outbound_intents(&output.outbound_intents)?;
    validate_event_intents(&output.event_intents)?;
    #[cfg(feature = "event-rings")]
    validate_event_reads(&output.outbound_event_reads, output.outbound_intents.len())?;
    let mut seen = Vec::new();
    for callback in &output.callback_intents {
        if callback.handler_method_id == 0
            || usize::try_from(callback.outbound_index)
                .ok()
                .is_none_or(|index| index >= output.outbound_intents.len())
            || seen.contains(&callback.outbound_index)
        {
            return Err(EncodeError::InvalidCallback);
        }
        seen.push(callback.outbound_index);
    }
    if output.outbound_intents.iter().any(|intent| intent.input_index != 0)
        || output.event_intents.iter().any(|intent| intent.input_index != 0)
    {
        return Err(EncodeError::InvalidIndex);
    }
    Ok(())
}

fn validate_batch_shape(
    output: &BatchOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<(), EncodeError> {
    if required_mode.is_some_and(|mode| mode != output.state_update.mode()) {
        return Err(EncodeError::StateModeMismatch);
    }
    validate_state_update(Some(&output.state_update))?;
    for status_code in &output.status_codes {
        validate_status_code(*status_code)?;
    }
    validate_outbound_intents(&output.outbound_intents)?;
    validate_event_intents(&output.event_intents)?;
    #[cfg(feature = "event-rings")]
    validate_event_reads(&output.outbound_event_reads, output.outbound_intents.len())?;
    Ok(())
}

fn validate_message_context(
    input: &MessageInput<'_>,
    is_init: bool,
    output: &MessageOutput,
) -> Result<(), EncodeError> {
    if is_init {
        if !input.message.is_deploy()
            || !output.callback_result_body.is_empty()
            || !output.callback_intents.is_empty()
            || output.code_upgrade_intent.is_some()
        {
            return Err(EncodeError::InvalidCallback);
        }
        return Ok(());
    }
    if input.message.is_deploy() {
        return Err(EncodeError::InvalidCallback);
    }
    if !input.message.has_callback_request() && !output.callback_result_body.is_empty() {
        return Err(EncodeError::InvalidCallback);
    }
    let upgrade_call = input.message.method_id() == CODE_UPGRADE_METHOD_ID;
    if output.status_code == 0 && upgrade_call && output.code_upgrade_intent.is_none() {
        return Err(EncodeError::UpgradeRequired);
    }
    if !upgrade_call && output.code_upgrade_intent.is_some() {
        return Err(EncodeError::UpgradeForbidden);
    }
    Ok(())
}

fn validate_batch_context(input: &BatchInput<'_>, output: &BatchOutput) -> Result<(), EncodeError> {
    if usize::try_from(input.message_count()).ok() != Some(output.status_codes.len()) {
        return Err(EncodeError::InvalidIndex);
    }
    for intent in &output.outbound_intents {
        validate_batch_effect_index(intent.input_index, &output.status_codes)?;
    }
    for intent in &output.event_intents {
        validate_batch_effect_index(intent.input_index, &output.status_codes)?;
    }
    Ok(())
}

fn validate_batch_effect_index(index: u32, statuses: &[u32]) -> Result<(), EncodeError> {
    let index = usize::try_from(index).map_err(|_| EncodeError::InvalidIndex)?;
    if statuses.get(index).copied() != Some(0) {
        return Err(EncodeError::InvalidIndex);
    }
    Ok(())
}

fn validate_status_code(status_code: u32) -> Result<(), EncodeError> {
    if (1..1000).contains(&status_code) {
        Err(EncodeError::InvalidStatusCode)
    } else {
        Ok(())
    }
}

fn validate_state_update(update: Option<&StateUpdate>) -> Result<(), EncodeError> {
    let Some(StateUpdate::DataPatchList(patches)) = update else {
        return Ok(());
    };
    let mut previous_end = 0u32;
    for patch in patches {
        let end = patch.offset.checked_add(patch.old_len).ok_or(EncodeError::InvalidPatchOrder)?;
        if patch.offset < previous_end {
            return Err(EncodeError::InvalidPatchOrder);
        }
        len_u32(patch.new_bytes.len())?;
        previous_end = end;
    }
    u16::try_from(patches.len()).map_err(|_| EncodeError::LengthOverflow)?;
    Ok(())
}

fn validate_outbound_intents(intents: &[OutboundIntent]) -> Result<(), EncodeError> {
    len_u32(intents.len())?;
    for intent in intents {
        if intent.destination.dapp_id == [0; 32] || intent.destination.account_id == [0; 32] {
            return Err(EncodeError::InvalidIndex);
        }
        validate_extra(&intent.extra)?;
        len_u32(intent.body.len())?;
    }
    Ok(())
}

/// Each entry names an outbound intent of the output, after the outbound
/// intent of the previous entry, with reads strictly ascending by source
/// DApp id, source account id and `event_class`. The host checks the count
/// limit of the network.
#[cfg(feature = "event-rings")]
fn validate_event_reads(
    entries: &[EventReadEntry],
    outbound_count: usize,
) -> Result<(), EncodeError> {
    len_u32(entries.len())?;
    let mut first_free_index = 0;
    for entry in entries {
        let index =
            usize::try_from(entry.outbound_index).map_err(|_| EncodeError::InvalidEventReads)?;
        if index < first_free_index
            || index >= outbound_count
            || entry.reads.is_empty()
            || entry.reads.windows(2).any(|pair| read_key(&pair[0]) >= read_key(&pair[1]))
        {
            return Err(EncodeError::InvalidEventReads);
        }
        len_u16(entry.reads.len())?;
        first_free_index = index + 1;
    }
    Ok(())
}

#[cfg(feature = "event-rings")]
fn read_key(read: &InternalEventReadRequest) -> (&[u8; 32], &[u8; 32], u64) {
    (&read.source.dapp_id, &read.source.account_id, read.event_class)
}

#[cfg(feature = "event-rings")]
fn has_event_reads(output: &MessageOutput) -> bool {
    !output.outbound_event_reads.is_empty()
}

#[cfg(not(feature = "event-rings"))]
fn has_event_reads(_: &MessageOutput) -> bool {
    false
}

fn validate_extra(extra: &[CurrencyAmount]) -> Result<(), EncodeError> {
    u16::try_from(extra.len()).map_err(|_| EncodeError::LengthOverflow)?;
    if extra.windows(2).any(|pair| pair[0].currency_id >= pair[1].currency_id) {
        return Err(EncodeError::InvalidExtraCurrencies);
    }
    Ok(())
}

fn validate_event_intents(intents: &[EventIntent]) -> Result<(), EncodeError> {
    len_u32(intents.len())?;
    for intent in intents {
        len_u16(intent.event_name.len())?;
        u16::try_from(intent.topics.len()).map_err(|_| EncodeError::LengthOverflow)?;
        for topic in &intent.topics {
            len_u16(topic.len())?;
        }
        len_u32(intent.fields_binary.len())?;
        validate_event_field_list(&intent.fields_binary)?;
    }
    Ok(())
}

/// Writes the state update. `flags` is the byte after its kind: zero in a
/// `MessageOutput`, `BatchOutput.flags` in a `BatchOutput`.
fn encode_state_update(
    bytes: &mut impl OutputWriter,
    update: &StateUpdate,
    flags: u8,
) -> Result<(), EncodeError> {
    bytes.write(&[update.mode() as u8, flags, 0, 0])?;
    match update {
        StateUpdate::FullReplace(state) => push_length_prefixed_u32(bytes, state),
        StateUpdate::DataPatchList(patches) => {
            bytes.write(&len_u16(patches.len())?.to_le_bytes())?;
            bytes.write(&0u16.to_le_bytes())?;
            for patch in patches {
                bytes.write(&patch.offset.to_le_bytes())?;
                bytes.write(&patch.old_len.to_le_bytes())?;
                bytes.write(&len_u32(patch.new_bytes.len())?.to_le_bytes())?;
                bytes.write(&patch.new_bytes)?;
            }
            Ok(())
        }
    }
}

fn encode_outbound_intents(
    bytes: &mut impl OutputWriter,
    intents: &[OutboundIntent],
) -> Result<(), EncodeError> {
    bytes.write(&len_u32(intents.len())?.to_le_bytes())?;
    for intent in intents {
        bytes.write(&intent.input_index.to_le_bytes())?;
        bytes.write(&intent.destination.dapp_id)?;
        bytes.write(&intent.destination.account_id)?;
        bytes.write(&intent.fuel_fee_shells.to_le_bytes())?;
        bytes.write(&len_u16(intent.extra.len())?.to_le_bytes())?;
        for amount in &intent.extra {
            bytes.write(&amount.currency_id.to_le_bytes())?;
            bytes.write(&amount.amount.to_le_bytes())?;
        }
        bytes.write(&intent.method_id.to_le_bytes())?;
        push_length_prefixed_u32(bytes, &intent.body)?;
    }
    Ok(())
}

fn encode_event_intents(
    bytes: &mut impl OutputWriter,
    intents: &[EventIntent],
) -> Result<(), EncodeError> {
    bytes.write(&len_u32(intents.len())?.to_le_bytes())?;
    for intent in intents {
        bytes.write(&intent.input_index.to_le_bytes())?;
        bytes.write(&[intent.kind as u8])?;
        bytes.write(&intent.event_class.to_le_bytes())?;
        bytes.write(&len_u16(intent.event_name.len())?.to_le_bytes())?;
        bytes.write(intent.event_name.as_bytes())?;
        bytes.write(&len_u16(intent.topics.len())?.to_le_bytes())?;
        for topic in &intent.topics {
            bytes.write(&len_u16(topic.len())?.to_le_bytes())?;
            bytes.write(topic)?;
        }
        push_length_prefixed_u32(bytes, &intent.fields_binary)?;
    }
    Ok(())
}

/// Writes `OutboundEventReads`; an output without entries has no section.
#[cfg(feature = "event-rings")]
fn encode_event_reads(
    bytes: &mut impl OutputWriter,
    entries: &[EventReadEntry],
) -> Result<(), EncodeError> {
    if entries.is_empty() {
        return Ok(());
    }
    bytes.write(&len_u32(entries.len())?.to_le_bytes())?;
    for entry in entries {
        bytes.write(&entry.outbound_index.to_le_bytes())?;
        bytes.write(&len_u16(entry.reads.len())?.to_le_bytes())?;
        bytes.write(&0u16.to_le_bytes())?;
        for read in &entry.reads {
            bytes.write(&read.source.dapp_id)?;
            bytes.write(&read.source.account_id)?;
            bytes.write(&read.event_class.to_le_bytes())?;
        }
    }
    Ok(())
}

fn encode_event_value(value: &EventValue) -> Result<(u8, Vec<u8>), EncodeError> {
    let encoded = match value {
        EventValue::Null => (0, Vec::new()),
        EventValue::Bool(value) => (1, alloc::vec![u8::from(*value)]),
        EventValue::U64(value) => (2, value.to_le_bytes().to_vec()),
        EventValue::I64(value) => (3, value.to_le_bytes().to_vec()),
        EventValue::U128(value) => (4, value.to_le_bytes().to_vec()),
        EventValue::I128(value) => (5, value.to_le_bytes().to_vec()),
        EventValue::Text(value) => (6, value.as_bytes().to_vec()),
        EventValue::Data(value) => (7, value.clone()),
        EventValue::Address(value) => {
            let mut bytes = Vec::with_capacity(64);
            bytes.extend_from_slice(&value.dapp_id);
            bytes.extend_from_slice(&value.account_id);
            (8, bytes)
        }
    };
    len_u16(encoded.1.len())?;
    Ok(encoded)
}

fn validate_event_field_list(bytes: &[u8]) -> Result<(), EncodeError> {
    let mut reader = FieldReader::new(bytes);
    let count = reader.u16()?;
    if reader.u16()? != 0 {
        return Err(EncodeError::InvalidEvent);
    }
    let mut previous_name: Option<&[u8]> = None;
    for _ in 0..count {
        let name_len = usize::from(reader.u16()?);
        let kind = reader.u8()?;
        let value_len = usize::from(reader.u16()?);
        let name = reader.take(name_len)?;
        core::str::from_utf8(name).map_err(|_| EncodeError::InvalidEvent)?;
        if previous_name.is_some_and(|previous| previous >= name) {
            return Err(EncodeError::InvalidEvent);
        }
        previous_name = Some(name);
        let value = reader.take(value_len)?;
        match kind {
            0 if value.is_empty() => {}
            1 if matches!(value, [0] | [1]) => {}
            2 | 3 if value.len() == 8 => {}
            4 | 5 if value.len() == 16 => {}
            6 if core::str::from_utf8(value).is_ok() => {}
            7 => {}
            8 if value.len() == 64 => {}
            _ => return Err(EncodeError::InvalidEvent),
        }
    }
    reader.finish()
}

fn push_length_prefixed_u32(
    bytes: &mut impl OutputWriter,
    value: &[u8],
) -> Result<(), EncodeError> {
    bytes.write(&len_u32(value.len())?.to_le_bytes())?;
    bytes.write(value)
}

fn len_u16(length: usize) -> Result<u16, EncodeError> {
    u16::try_from(length).map_err(|_| EncodeError::LengthOverflow)
}

fn len_u32(length: usize) -> Result<u32, EncodeError> {
    u32::try_from(length).map_err(|_| EncodeError::LengthOverflow)
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct FieldReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> FieldReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], EncodeError> {
        let end = self.position.checked_add(length).ok_or(EncodeError::LengthOverflow)?;
        let value = self.bytes.get(self.position..end).ok_or(EncodeError::InvalidEvent)?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, EncodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, EncodeError> {
        let bytes = self.take(2)?.try_into().map_err(|_| EncodeError::InvalidEvent)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn finish(self) -> Result<(), EncodeError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(EncodeError::InvalidEvent)
        }
    }
}

#[cfg(test)]
pub(crate) mod work {
    extern crate std;
    use core::cell::Cell;

    std::thread_local! {
        static WRITES: Cell<(usize, usize, usize)> = const { Cell::new((0, 0, 0)) };
    }

    pub(super) fn vec_write(bytes: usize, grew: bool) {
        WRITES.with(|work| {
            let (written, growths, arena) = work.get();
            work.set((written + bytes, growths + usize::from(grew), arena));
        });
    }

    /// Bytes written into a preallocated output, as the guest arena writes them.
    pub(super) fn direct_write(bytes: usize) {
        WRITES.with(|work| {
            let (written, growths, direct) = work.get();
            work.set((written, growths, direct + bytes));
        });
    }

    pub(crate) fn take() -> (usize, usize, usize) {
        WRITES.with(|work| work.replace((0, 0, 0)))
    }

    #[test]
    fn checked_sizes_reject_arithmetic_overflow() {
        use super::*;
        assert_eq!(checked_len(&[usize::MAX, 1]), Err(EncodeError::LengthOverflow));
        assert_eq!(repeated_len(usize::MAX, 2), Err(EncodeError::LengthOverflow));
        assert_eq!(checked_len(&[usize::MAX, 0]), Ok(usize::MAX));
    }
}

#[cfg(test)]
mod writer_tests {
    // The output written straight into a preallocated buffer of the exact length,
    // as the WASM entrypoint dispatch writes it into the guest arena. The arena
    // itself is tested in real WASM (SDK docs/unsafe.md).
    use super::*;
    use crate::{
        decode_batch_input, decode_message_input, BatchContract, Contract, MessageInput,
        MessageOutput, StateUpdate,
    };
    use alloc::vec;

    /// A preallocated output buffer that accepts exactly its length.
    struct Exact<'a>(&'a mut [u8]);

    impl OutputWriter for Exact<'_> {
        fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
            let (destination, rest) = core::mem::take(&mut self.0)
                .split_at_mut_checked(bytes.len())
                .ok_or(EncodeError::LengthOverflow)?;
            destination.copy_from_slice(bytes);
            self.0 = rest;
            work::direct_write(bytes.len());
            Ok(())
        }
    }

    /// The `Vec<u8>` writer of the public serializer, with its writes and
    /// growths counted.
    struct Growing(Vec<u8>);

    impl OutputWriter for Growing {
        fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
            let capacity = self.0.capacity();
            self.0.write(bytes)?;
            work::vec_write(bytes.len(), self.0.capacity() != capacity);
            Ok(())
        }
    }

    /// The public serializer path, `write_message_output` into a growing `Vec<u8>`,
    /// checked against `encode_message_output`.
    fn public(output: &MessageOutput) -> Vec<u8> {
        let mut writer = Growing(Vec::new());
        write_message_output(&mut writer, output).unwrap();
        assert_eq!(writer.0, crate::encode_message_output(output).unwrap());
        writer.0
    }

    /// Writes an output of the computed `len` and checks that it fills the buffer.
    fn direct(
        len: usize,
        write: impl FnOnce(&mut Exact<'_>) -> Result<(), EncodeError>,
    ) -> Vec<u8> {
        let mut buffer = vec![0; len];
        let mut writer = Exact(&mut buffer);
        write(&mut writer).unwrap();
        assert!(writer.0.is_empty(), "the output fills its computed length");
        buffer
    }

    /// The output half of a message dispatch: exact length, then one direct write.
    fn write_message<T: Contract>(bytes: &[u8]) -> Vec<u8> {
        let input = decode_message_input(bytes).unwrap();
        let output = T::process_message(input);
        let len = message_output_len_for(&input, false, &output).unwrap();
        direct(len, |writer| write_message_output(writer, &output))
    }

    struct Echo;
    impl Contract for Echo {
        fn init(input: MessageInput<'_>) -> MessageOutput {
            Self::process_message(input)
        }

        fn process_message(input: MessageInput<'_>) -> MessageOutput {
            MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
        }
    }

    fn input() -> Vec<u8> {
        // Canonical ACMI with one internal message, no extras or optional sections.
        let mut bytes = vec![0; 262];
        bytes[..4].copy_from_slice(b"ACMI");
        bytes[60..124].fill(1);
        bytes[156..220].fill(2);
        bytes[220..252].fill(3);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"old");
        bytes.extend_from_slice(&176u32.to_le_bytes());
        let mut message = vec![0; 176];
        message[8..16].copy_from_slice(&1u64.to_le_bytes());
        message[48..112].fill(1);
        message[112..176].fill(2);
        bytes.extend_from_slice(&message);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        decode_message_input(&bytes).unwrap();
        bytes
    }

    #[test]
    fn direct_message_output_writes_once_without_the_vec_growth_path() {
        let bytes = input();
        let expected = public(&Echo::process_message(decode_message_input(&bytes).unwrap()));
        let (vec_bytes, growths, _) = work::take();
        assert_eq!(vec_bytes, expected.len());
        assert!(growths > 1, "the public Vec serializer is a live positive control");
        assert_eq!(write_message::<Echo>(&bytes), expected);
        assert_eq!(work::take(), (0, 0, expected.len()));
    }

    /// A minter output as the mint dispatch writes it: its computed length,
    /// then one direct write, equal to `encode_mint_output`.
    #[cfg(feature = "system-mint")]
    #[test]
    fn direct_mint_output_fills_its_computed_length() {
        let bytes = input();
        let message = decode_message_input(&bytes).unwrap();
        let output = Echo::process_message(message);
        let mint = [CurrencyAmount { currency_id: 2, amount: u128::MAX }];
        for mints in [&[][..], &mint[..]] {
            let len = mint_output_len_for(&message, &output, mints).unwrap();
            let written = direct(len, |writer| write_mint_output(writer, &output, mints));
            assert_eq!(written, crate::encode_mint_output(&output, mints).unwrap());
        }
    }

    #[test]
    fn ten_thousand_wallet_sized_outputs_remove_observed_vec_growth_and_copy() {
        // A work-count fixture, not a replacement for the registered Wallet benchmark.
        struct OutputShape;
        impl Contract for OutputShape {
            fn init(input: MessageInput<'_>) -> MessageOutput {
                Self::process_message(input)
            }
            fn process_message(input: MessageInput<'_>) -> MessageOutput {
                let mut output = MessageOutput::success(StateUpdate::FullReplace(vec![1; 144]));
                output.outbound_intents.push(crate::OutboundIntent {
                    input_index: 0,
                    destination: input.destination,
                    fuel_fee_shells: 1,
                    extra: Vec::new(),
                    method_id: 1,
                    body: vec![2; 64],
                });
                output
            }
        }
        let bytes = input();
        work::take();
        for _ in 0..10_000 {
            let output = OutputShape::process_message(decode_message_input(&bytes).unwrap());
            let expected = public(&output);
            assert_eq!(expected.len(), 339);
            assert_eq!(write_message::<OutputShape>(&bytes), expected);
        }
        // Each positive-control Vec grows six times (first allocation included).
        // The direct write adds no Vec writes/growth; only one direct write/byte.
        assert_eq!(work::take(), (3_390_000, 60_000, 3_390_000));
    }

    #[test]
    fn batch_output_preserves_count_mode_and_failed_index_checks() {
        struct BatchEcho;
        impl Contract for BatchEcho {
            fn init(input: MessageInput<'_>) -> MessageOutput {
                Echo::init(input)
            }
            fn process_message(input: MessageInput<'_>) -> MessageOutput {
                Echo::process_message(input)
            }
        }
        impl BatchContract for BatchEcho {
            fn process_batch(input: crate::BatchInput<'_>) -> crate::BatchOutput {
                crate::BatchOutput {
                    state_update: StateUpdate::FullReplace(input.state.to_vec()),
                    status_codes: vec![0, 1000],
                    outbound_intents: Vec::new(),
                    event_intents: Vec::new(),
                    #[cfg(feature = "event-rings")]
                    outbound_event_reads: Vec::new(),
                }
            }
        }
        let message = input();
        let mut bytes = vec![0; 158];
        bytes[..4].copy_from_slice(b"ACBI");
        bytes[52..116].fill(2);
        bytes[116..148].fill(3);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for offset in [0u32, 176, 352] {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend_from_slice(&message[273..449]);
        bytes.extend_from_slice(&message[273..449]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let input = decode_batch_input(&bytes).unwrap();
        let mut output = BatchEcho::process_batch(input);
        let expected = crate::encode_batch_output(&output).unwrap();
        assert_eq!(batch_output_len_for(&input, &output).unwrap(), expected.len());
        let len = batch_output_len_for(&input, &output).unwrap();
        assert_eq!(direct(len, |writer| write_batch_output(writer, &output)), expected);
        output.status_codes.pop();
        output.state_update = StateUpdate::DataPatchList(Vec::new());
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::InvalidIndex));
        output.status_codes.push(1000);
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::StateModeMismatch));
        output.outbound_intents.push(crate::OutboundIntent {
            input_index: 1,
            destination: input.destination,
            fuel_fee_shells: 0,
            extra: Vec::new(),
            method_id: 1,
            body: Vec::new(),
        });
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::InvalidIndex));
    }

    #[cfg(feature = "event-rings")]
    #[test]
    fn direct_outbound_event_reads_match_the_public_serializer() {
        use crate::{AccountAddress, BatchOutput, EventReadEntry, InternalEventReadRequest};
        let bytes = input();
        let input = decode_message_input(&bytes).unwrap();
        let mut output = MessageOutput::success(StateUpdate::FullReplace(vec![1]));
        output.outbound_intents.push(crate::OutboundIntent {
            input_index: 0,
            destination: input.destination,
            fuel_fee_shells: 2,
            extra: Vec::new(),
            method_id: 3,
            body: vec![4; 5],
        });
        let read = |event_class| InternalEventReadRequest {
            source: AccountAddress { dapp_id: [6; 32], account_id: [7; 32] },
            event_class,
        };
        output.outbound_event_reads =
            vec![EventReadEntry { outbound_index: 0, reads: vec![read(8), read(9)] }];
        let expected = crate::encode_message_output(&output).unwrap();
        let len = message_output_len_for(&input, false, &output).unwrap();
        assert_eq!(len, expected.len());
        assert_eq!(direct(len, |writer| write_message_output(writer, &output)), expected);

        let batch = BatchOutput {
            state_update: StateUpdate::FullReplace(vec![1]),
            status_codes: vec![0],
            outbound_intents: output.outbound_intents,
            event_intents: Vec::new(),
            outbound_event_reads: output.outbound_event_reads,
        };
        let expected = crate::encode_batch_output(&batch).unwrap();
        let len = batch_output_len(&batch).unwrap();
        assert_eq!(len, expected.len());
        assert_eq!(direct(len, |writer| write_batch_output(writer, &batch)), expected);
    }

    #[cfg(feature = "event-rings")]
    #[test]
    fn direct_message_sections_and_batch_match_the_public_serializer() {
        use crate::{
            BatchOutput, CallbackIntent, CanonicalMessage, CodeUpgradeIntent, CurrencyAmount,
            EventIntent, EventKind, OutboundIntent, StateOutputMode, StatePatch,
        };
        let bytes = input();
        let mut input = decode_message_input(&bytes).unwrap();
        let CanonicalMessage::Internal(ref mut message) = input.message else { panic!() };
        message.flags |= 4;
        message.method_id = crate::CODE_UPGRADE_METHOD_ID;
        for state in [
            StateUpdate::FullReplace(vec![5; 144]),
            StateUpdate::DataPatchList(vec![StatePatch {
                offset: 1,
                old_len: 1,
                new_bytes: vec![6; 4096],
            }]),
            StateUpdate::DataPatchList(Vec::new()),
        ] {
            input.required_state_output_mode = state.mode();
            let mut output = MessageOutput::success(state.clone());
            output.callback_result_body = vec![7; 19];
            output.outbound_intents.push(OutboundIntent {
                input_index: 0,
                destination: input.destination,
                fuel_fee_shells: 9,
                extra: vec![
                    CurrencyAmount { currency_id: 0, amount: 8 },
                    CurrencyAmount { currency_id: 1, amount: 10 },
                ],
                method_id: 11,
                body: vec![12; 64],
            });
            output
                .callback_intents
                .push(CallbackIntent { outbound_index: 0, handler_method_id: 13 });
            output.event_intents.push(EventIntent {
                input_index: 0,
                kind: EventKind::Internal,
                event_class: 14,
                event_name: "changed".into(),
                topics: vec![vec![15; 17], Vec::new()],
                fields_binary: crate::encode_event_fields(&[]).unwrap(),
            });
            output.code_upgrade_intent = Some(CodeUpgradeIntent {
                target_selected_code_id: [16; 32],
                expected_code_hash: [17; 32],
            });
            let expected = crate::encode_message_output(&output).unwrap();
            let len = message_output_len_for(&input, false, &output).unwrap();
            assert_eq!(len, expected.len());
            assert_eq!(direct(len, |writer| write_message_output(writer, &output)), expected);

            let batch = BatchOutput {
                state_update: state,
                status_codes: vec![0, 1000],
                outbound_intents: output.outbound_intents,
                event_intents: output.event_intents,
                outbound_event_reads: output.outbound_event_reads,
            };
            let expected = crate::encode_batch_output(&batch).unwrap();
            assert_eq!(
                direct(expected.len(), |writer| write_batch_output(writer, &batch)),
                expected
            );
        }
        let mut invalid = MessageOutput::failure(7);
        invalid.callback_result_body.push(1);
        assert_eq!(
            message_output_len_for(&input, true, &invalid),
            Err(EncodeError::InvalidCallback)
        );
        assert_eq!(
            message_output_len_for(&input, false, &invalid),
            Err(EncodeError::InvalidStatusCode)
        );
        input.required_state_output_mode = StateOutputMode::DataPatchList;
        let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
        assert_eq!(
            message_output_len_for(&input, false, &output),
            Err(EncodeError::UpgradeRequired)
        );
        output.code_upgrade_intent = Some(CodeUpgradeIntent {
            target_selected_code_id: [1; 32],
            expected_code_hash: [2; 32],
        });
        assert_eq!(
            message_output_len_for(&input, false, &output),
            Err(EncodeError::StateModeMismatch)
        );
    }
}
