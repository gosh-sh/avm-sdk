use alloc::{string::String, vec::Vec};

use crate::{
    AccountAddress, AvmAmount, BatchInput, CodeHash, CurrencyAmount, InternalEventReadRequest,
    MessageInput, SelectedCodeId, StateOutputMode, CODE_UPGRADE_METHOD_ID,
};

const FLAG_HAS_STATE_UPDATE: u8 = 1;
const FLAG_HAS_UPGRADE_INTENT: u8 = 1 << 1;

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
    InvalidEventReads,
    UpgradeForbidden,
    UpgradeRequired,
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
    pub value_main: AvmAmount,
    pub fuel_fee_shells: AvmAmount,
    pub extra: Vec<CurrencyAmount>,
    /// Exact event-ring keys to resolve for the recipient at its block start.
    /// Nonempty requests require the experimental ACM2/ACB2 runtime ABI.
    pub internal_event_reads: Vec<InternalEventReadRequest>,
    pub method_id: u64,
    pub body: Vec<u8>,
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
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchOutput {
    pub state_update: StateUpdate,
    pub status_codes: Vec<u32>,
    pub outbound_intents: Vec<OutboundIntent>,
    pub event_intents: Vec<EventIntent>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputPlan {
    pub len: usize,
    pub extended: bool,
}

pub(crate) fn message_output_plan_for(
    input: &MessageInput<'_>,
    is_init: bool,
    output: &MessageOutput,
) -> Result<OutputPlan, EncodeError> {
    validate_message_context(input, is_init, output)?;
    let extended = validate_message_shape(output, Some(input.required_state_output_mode))?;
    Ok(OutputPlan { len: message_output_len(output, extended)?, extended })
}

pub(crate) fn batch_output_plan_for(
    input: &BatchInput<'_>,
    output: &BatchOutput,
) -> Result<OutputPlan, EncodeError> {
    validate_batch_context(input, output)?;
    let extended = validate_batch_shape(output, Some(input.required_state_output_mode))?;
    Ok(OutputPlan { len: batch_output_len(output, extended)?, extended })
}

pub(crate) trait OutputWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError>;
}

impl OutputWriter for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        #[cfg(test)]
        let capacity = self.capacity();
        self.extend_from_slice(bytes);
        #[cfg(test)]
        work::vec_write(bytes.len(), self.capacity() != capacity);
        Ok(())
    }
}

fn encode_message_output_inner(
    output: &MessageOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    let extended = validate_message_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    write_message_output(&mut bytes, output, extended)?;
    Ok(bytes)
}

pub(crate) fn write_message_output(
    bytes: &mut impl OutputWriter,
    output: &MessageOutput,
    extended: bool,
) -> Result<(), EncodeError> {
    bytes.write(if extended { b"ACM2" } else { b"ACMO" })?;
    bytes.write(&output.status_code.to_le_bytes())?;

    let mut flags = 0;
    if output.state_update.is_some() {
        flags |= FLAG_HAS_STATE_UPDATE;
    }
    if output.code_upgrade_intent.is_some() {
        flags |= FLAG_HAS_UPGRADE_INTENT;
    }
    bytes.write(&[flags])?;
    if let Some(state_update) = &output.state_update {
        encode_state_update(bytes, state_update)?;
    }
    push_length_prefixed_u32(bytes, &output.callback_result_body)?;
    encode_outbound_intents(bytes, &output.outbound_intents, extended)?;
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
    Ok(())
}

fn encode_batch_output_inner(
    output: &BatchOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    let extended = validate_batch_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    write_batch_output(&mut bytes, output, extended)?;
    Ok(bytes)
}

pub(crate) fn write_batch_output(
    bytes: &mut impl OutputWriter,
    output: &BatchOutput,
    extended: bool,
) -> Result<(), EncodeError> {
    bytes.write(if extended { b"ACB2" } else { b"ACBO" })?;
    encode_state_update(bytes, &output.state_update)?;
    bytes.write(&len_u32(output.status_codes.len())?.to_le_bytes())?;
    for status_code in &output.status_codes {
        bytes.write(&status_code.to_le_bytes())?;
    }
    encode_outbound_intents(bytes, &output.outbound_intents, extended)?;
    encode_event_intents(bytes, &output.event_intents)?;
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

fn outbound_intents_len(intents: &[OutboundIntent], extended: bool) -> Result<usize, EncodeError> {
    intents.iter().try_fold(4, |total, intent| {
        checked_len(&[
            total,
            114,
            repeated_len(intent.extra.len(), 20)?,
            intent.body.len(),
            if extended {
                checked_len(&[4, repeated_len(intent.internal_event_reads.len(), 72)?])?
            } else {
                0
            },
        ])
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

fn message_output_len(output: &MessageOutput, extended: bool) -> Result<usize, EncodeError> {
    checked_len(&[
        9,
        output.state_update.as_ref().map(state_update_len).transpose()?.unwrap_or(0),
        4,
        output.callback_result_body.len(),
        outbound_intents_len(&output.outbound_intents, extended)?,
        4,
        repeated_len(output.callback_intents.len(), 12)?,
        event_intents_len(&output.event_intents)?,
        if output.code_upgrade_intent.is_some() { 64 } else { 0 },
    ])
}

fn batch_output_len(output: &BatchOutput, extended: bool) -> Result<usize, EncodeError> {
    checked_len(&[
        4,
        state_update_len(&output.state_update)?,
        4,
        repeated_len(output.status_codes.len(), 4)?,
        outbound_intents_len(&output.outbound_intents, extended)?,
        event_intents_len(&output.event_intents)?,
    ])
}

fn validate_message_shape(
    output: &MessageOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<bool, EncodeError> {
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
    {
        return Err(EncodeError::EffectsOnFailure);
    }

    validate_state_update(output.state_update.as_ref())?;
    let extended = validate_outbound_intents(&output.outbound_intents)?;
    validate_event_intents(&output.event_intents)?;
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
    Ok(extended)
}

fn validate_batch_shape(
    output: &BatchOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<bool, EncodeError> {
    if required_mode.is_some_and(|mode| mode != output.state_update.mode()) {
        return Err(EncodeError::StateModeMismatch);
    }
    validate_state_update(Some(&output.state_update))?;
    for status_code in &output.status_codes {
        validate_status_code(*status_code)?;
    }
    let extended = validate_outbound_intents(&output.outbound_intents)?;
    validate_event_intents(&output.event_intents)?;
    Ok(extended)
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

fn validate_outbound_intents(intents: &[OutboundIntent]) -> Result<bool, EncodeError> {
    len_u32(intents.len())?;
    let mut extended = false;
    for intent in intents {
        #[cfg(test)]
        work::event_read_probe();
        extended |= !intent.internal_event_reads.is_empty();
        if intent.destination.dapp_id == [0; 32] || intent.destination.account_id == [0; 32] {
            return Err(EncodeError::InvalidIndex);
        }
        validate_extra(&intent.extra)?;
        len_u16(intent.internal_event_reads.len())?;
        if intent.internal_event_reads.windows(2).any(|pair| {
            let key = |read: &InternalEventReadRequest| {
                (read.source.dapp_id, read.source.account_id, read.event_class)
            };
            key(&pair[0]) >= key(&pair[1])
        }) {
            return Err(EncodeError::InvalidEventReads);
        }
        len_u32(intent.body.len())?;
    }
    Ok(extended)
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

fn encode_state_update(
    bytes: &mut impl OutputWriter,
    update: &StateUpdate,
) -> Result<(), EncodeError> {
    bytes.write(&[update.mode() as u8])?;
    bytes.write(&[0; 3])?;
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
    extended: bool,
) -> Result<(), EncodeError> {
    bytes.write(&len_u32(intents.len())?.to_le_bytes())?;
    for intent in intents {
        bytes.write(&intent.input_index.to_le_bytes())?;
        bytes.write(&intent.destination.dapp_id)?;
        bytes.write(&intent.destination.account_id)?;
        bytes.write(&intent.value_main.to_le_bytes())?;
        bytes.write(&intent.fuel_fee_shells.to_le_bytes())?;
        bytes.write(&len_u16(intent.extra.len())?.to_le_bytes())?;
        for amount in &intent.extra {
            bytes.write(&amount.currency_id.to_le_bytes())?;
            bytes.write(&amount.amount.to_le_bytes())?;
        }
        if extended {
            bytes.write(&len_u16(intent.internal_event_reads.len())?.to_le_bytes())?;
            bytes.write(&0u16.to_le_bytes())?;
            for read in &intent.internal_event_reads {
                bytes.write(&read.source.dapp_id)?;
                bytes.write(&read.source.account_id)?;
                bytes.write(&read.event_class.to_le_bytes())?;
            }
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
        static EVENT_READ_PROBES: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn event_read_probe() {
        EVENT_READ_PROBES.with(|count| count.set(count.get() + 1));
    }

    pub(crate) fn take_event_read_probes() -> usize {
        EVENT_READ_PROBES.with(|count| count.replace(0))
    }

    pub(super) fn vec_write(bytes: usize, grew: bool) {
        WRITES.with(|work| {
            let (written, growths, arena) = work.get();
            work.set((written + bytes, growths + usize::from(grew), arena));
        });
    }

    pub(crate) fn arena_write(bytes: usize) {
        WRITES.with(|work| {
            let (written, growths, arena) = work.get();
            work.set((written, growths, arena + bytes));
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
