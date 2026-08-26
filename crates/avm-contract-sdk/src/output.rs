use alloc::{string::String, vec::Vec};

use crate::{
    AccountAddress, AvmAmount, BatchInput, CodeHash, CurrencyAmount, MessageInput, SelectedCodeId,
    StateOutputMode, CODE_UPGRADE_METHOD_ID,
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

pub(crate) fn encode_message_output_for(
    input: &MessageInput<'_>,
    is_init: bool,
    output: &MessageOutput,
) -> Result<Vec<u8>, EncodeError> {
    validate_message_context(input, is_init, output)?;
    encode_message_output_inner(output, Some(input.required_state_output_mode))
}

pub(crate) fn encode_batch_output_for(
    input: &BatchInput<'_>,
    output: &BatchOutput,
) -> Result<Vec<u8>, EncodeError> {
    validate_batch_context(input, output)?;
    encode_batch_output_inner(output, Some(input.required_state_output_mode))
}

fn encode_message_output_inner(
    output: &MessageOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    validate_message_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ACMO");
    push_u32(&mut bytes, output.status_code);

    let mut flags = 0;
    if output.state_update.is_some() {
        flags |= FLAG_HAS_STATE_UPDATE;
    }
    if output.code_upgrade_intent.is_some() {
        flags |= FLAG_HAS_UPGRADE_INTENT;
    }
    bytes.push(flags);
    if let Some(state_update) = &output.state_update {
        encode_state_update(&mut bytes, state_update)?;
    }
    push_length_prefixed_u32(&mut bytes, &output.callback_result_body)?;
    encode_outbound_intents(&mut bytes, &output.outbound_intents)?;
    push_u32(&mut bytes, len_u32(output.callback_intents.len())?);
    for callback in &output.callback_intents {
        push_u32(&mut bytes, callback.outbound_index);
        push_u64(&mut bytes, callback.handler_method_id);
    }
    encode_event_intents(&mut bytes, &output.event_intents)?;
    if let Some(upgrade) = &output.code_upgrade_intent {
        bytes.extend_from_slice(&upgrade.target_selected_code_id);
        bytes.extend_from_slice(&upgrade.expected_code_hash);
    }
    Ok(bytes)
}

fn encode_batch_output_inner(
    output: &BatchOutput,
    required_mode: Option<StateOutputMode>,
) -> Result<Vec<u8>, EncodeError> {
    validate_batch_shape(output, required_mode)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ACBO");
    encode_state_update(&mut bytes, &output.state_update)?;
    push_u32(&mut bytes, len_u32(output.status_codes.len())?);
    for status_code in &output.status_codes {
        push_u32(&mut bytes, *status_code);
    }
    encode_outbound_intents(&mut bytes, &output.outbound_intents)?;
    encode_event_intents(&mut bytes, &output.event_intents)?;
    Ok(bytes)
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
    {
        return Err(EncodeError::EffectsOnFailure);
    }

    validate_state_update(output.state_update.as_ref())?;
    validate_outbound_intents(&output.outbound_intents)?;
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
    validate_event_intents(&output.event_intents)
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

fn encode_state_update(bytes: &mut Vec<u8>, update: &StateUpdate) -> Result<(), EncodeError> {
    bytes.push(update.mode() as u8);
    bytes.extend_from_slice(&[0; 3]);
    match update {
        StateUpdate::FullReplace(state) => push_length_prefixed_u32(bytes, state),
        StateUpdate::DataPatchList(patches) => {
            push_u16(bytes, u16::try_from(patches.len()).map_err(|_| EncodeError::LengthOverflow)?);
            push_u16(bytes, 0);
            for patch in patches {
                push_u32(bytes, patch.offset);
                push_u32(bytes, patch.old_len);
                push_u32(bytes, len_u32(patch.new_bytes.len())?);
                bytes.extend_from_slice(&patch.new_bytes);
            }
            Ok(())
        }
    }
}

fn encode_outbound_intents(
    bytes: &mut Vec<u8>,
    intents: &[OutboundIntent],
) -> Result<(), EncodeError> {
    push_u32(bytes, len_u32(intents.len())?);
    for intent in intents {
        push_u32(bytes, intent.input_index);
        bytes.extend_from_slice(&intent.destination.dapp_id);
        bytes.extend_from_slice(&intent.destination.account_id);
        push_u128(bytes, intent.value_main);
        push_u128(bytes, intent.fuel_fee_shells);
        push_u16(
            bytes,
            u16::try_from(intent.extra.len()).map_err(|_| EncodeError::LengthOverflow)?,
        );
        for amount in &intent.extra {
            push_u32(bytes, amount.currency_id);
            push_u128(bytes, amount.amount);
        }
        push_u64(bytes, intent.method_id);
        push_length_prefixed_u32(bytes, &intent.body)?;
    }
    Ok(())
}

fn encode_event_intents(bytes: &mut Vec<u8>, intents: &[EventIntent]) -> Result<(), EncodeError> {
    push_u32(bytes, len_u32(intents.len())?);
    for intent in intents {
        push_u32(bytes, intent.input_index);
        bytes.push(intent.kind as u8);
        push_u64(bytes, intent.event_class);
        push_u16(bytes, len_u16(intent.event_name.len())?);
        bytes.extend_from_slice(intent.event_name.as_bytes());
        push_u16(
            bytes,
            u16::try_from(intent.topics.len()).map_err(|_| EncodeError::LengthOverflow)?,
        );
        for topic in &intent.topics {
            push_u16(bytes, len_u16(topic.len())?);
            bytes.extend_from_slice(topic);
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

fn push_length_prefixed_u32(bytes: &mut Vec<u8>, value: &[u8]) -> Result<(), EncodeError> {
    push_u32(bytes, len_u32(value.len())?);
    bytes.extend_from_slice(value);
    Ok(())
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

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u128(bytes: &mut Vec<u8>, value: u128) {
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
