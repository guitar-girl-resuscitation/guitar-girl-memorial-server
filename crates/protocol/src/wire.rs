use std::io::{Cursor, Read, Write};

use base64::{Engine, engine::general_purpose::STANDARD};
use bzip2::{Compression, read::BzDecoder, write::BzEncoder};

pub const STOP: u8 = 0;
pub const BOOL: u8 = 2;
pub const BYTE: u8 = 3;
pub const DOUBLE: u8 = 4;
pub const I16: u8 = 6;
pub const I32: u8 = 8;
pub const I64: u8 = 10;
pub const STRING: u8 = 11;
pub const STRUCT: u8 = 12;
pub const MAP: u8 = 13;
pub const SET: u8 = 14;
pub const LIST: u8 = 15;

const MAX_DECOMPRESSED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: i16,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Byte(i8),
    Double(f64),
    I16(i16),
    I32(i32),
    I64(i64),
    String(Vec<u8>),
    Struct(Vec<Field>),
    Map {
        key_type: u8,
        value_type: u8,
        entries: Vec<(Value, Value)>,
    },
    Set {
        element_type: u8,
        values: Vec<Value>,
    },
    List {
        element_type: u8,
        values: Vec<Value>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ThriftError {
    #[error("transport base64 is invalid: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("transport or thrift I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported thrift type {0}")]
    UnsupportedType(u8),
    #[error("invalid collection size {0}")]
    InvalidSize(i32),
    #[error("trailing thrift bytes: {0}")]
    TrailingBytes(usize),
    #[error("decoded thrift payload is empty")]
    EmptyPayload,
    #[error("decompressed payload exceeds 32 MiB")]
    PayloadTooLarge,
}

pub fn decode_transport(encoded: &str) -> Result<Vec<u8>, ThriftError> {
    let normalized = encoded.trim().replace(' ', "+");
    if normalized.is_empty() {
        return Err(ThriftError::EmptyPayload);
    }
    let compressed = STANDARD.decode(normalized)?;
    let mut decoder = BzDecoder::new(compressed.as_slice());
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take((MAX_DECOMPRESSED_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_DECOMPRESSED_BYTES {
        return Err(ThriftError::PayloadTooLarge);
    }
    if raw.is_empty() {
        return Err(ThriftError::EmptyPayload);
    }
    Ok(raw)
}

pub fn encode_transport(raw: &[u8]) -> Result<String, ThriftError> {
    if raw.is_empty() {
        return Err(ThriftError::EmptyPayload);
    }
    let mut encoder = BzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(raw)?;
    Ok(STANDARD.encode(encoder.finish()?))
}

pub fn read_struct(raw: &[u8]) -> Result<Vec<Field>, ThriftError> {
    let mut cursor = Cursor::new(raw);
    let fields = read_struct_from(&mut cursor)?;
    let trailing = raw.len() - cursor.position() as usize;
    if trailing != 0 {
        return Err(ThriftError::TrailingBytes(trailing));
    }
    Ok(fields)
}

pub fn write_struct(fields: &[Field]) -> Result<Vec<u8>, ThriftError> {
    let mut output = Vec::new();
    write_struct_to(&mut output, fields)?;
    Ok(output)
}

fn read_struct_from(input: &mut Cursor<&[u8]>) -> Result<Vec<Field>, ThriftError> {
    let mut fields = Vec::new();
    loop {
        let kind = read_u8(input)?;
        if kind == STOP {
            return Ok(fields);
        }
        let id = read_i16(input)?;
        fields.push(Field {
            id,
            value: read_value(input, kind)?,
        });
    }
}

fn read_value(input: &mut Cursor<&[u8]>, kind: u8) -> Result<Value, ThriftError> {
    Ok(match kind {
        BOOL => Value::Bool(read_u8(input)? != 0),
        BYTE => Value::Byte(read_u8(input)? as i8),
        DOUBLE => Value::Double(f64::from_bits(read_u64(input)?)),
        I16 => Value::I16(read_i16(input)?),
        I32 => Value::I32(read_i32(input)?),
        I64 => Value::I64(read_i64(input)?),
        STRING => {
            let length = checked_size(read_i32(input)?)?;
            let mut bytes = vec![0; length];
            input.read_exact(&mut bytes)?;
            Value::String(bytes)
        }
        STRUCT => Value::Struct(read_struct_from(input)?),
        MAP => {
            let key_type = read_u8(input)?;
            let value_type = read_u8(input)?;
            let count = checked_size(read_i32(input)?)?;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                entries.push((read_value(input, key_type)?, read_value(input, value_type)?));
            }
            Value::Map {
                key_type,
                value_type,
                entries,
            }
        }
        SET | LIST => {
            let element_type = read_u8(input)?;
            let count = checked_size(read_i32(input)?)?;
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(read_value(input, element_type)?);
            }
            if kind == SET {
                Value::Set {
                    element_type,
                    values,
                }
            } else {
                Value::List {
                    element_type,
                    values,
                }
            }
        }
        other => return Err(ThriftError::UnsupportedType(other)),
    })
}

fn write_struct_to(output: &mut Vec<u8>, fields: &[Field]) -> Result<(), ThriftError> {
    for field in fields {
        output.push(value_kind(&field.value));
        output.extend_from_slice(&field.id.to_be_bytes());
        write_value(output, &field.value)?;
    }
    output.push(STOP);
    Ok(())
}

fn write_value(output: &mut Vec<u8>, value: &Value) -> Result<(), ThriftError> {
    match value {
        Value::Bool(value) => output.push(u8::from(*value)),
        Value::Byte(value) => output.push(*value as u8),
        Value::Double(value) => output.extend_from_slice(&value.to_bits().to_be_bytes()),
        Value::I16(value) => output.extend_from_slice(&value.to_be_bytes()),
        Value::I32(value) => output.extend_from_slice(&value.to_be_bytes()),
        Value::I64(value) => output.extend_from_slice(&value.to_be_bytes()),
        Value::String(value) => {
            output.extend_from_slice(&(value.len() as i32).to_be_bytes());
            output.extend_from_slice(value);
        }
        Value::Struct(fields) => write_struct_to(output, fields)?,
        Value::Map {
            key_type,
            value_type,
            entries,
        } => {
            output.extend_from_slice(&[*key_type, *value_type]);
            output.extend_from_slice(&(entries.len() as i32).to_be_bytes());
            for (key, value) in entries {
                write_value(output, key)?;
                write_value(output, value)?;
            }
        }
        Value::Set {
            element_type,
            values,
        }
        | Value::List {
            element_type,
            values,
        } => {
            output.push(*element_type);
            output.extend_from_slice(&(values.len() as i32).to_be_bytes());
            for value in values {
                write_value(output, value)?;
            }
        }
    }
    Ok(())
}

fn value_kind(value: &Value) -> u8 {
    match value {
        Value::Bool(_) => BOOL,
        Value::Byte(_) => BYTE,
        Value::Double(_) => DOUBLE,
        Value::I16(_) => I16,
        Value::I32(_) => I32,
        Value::I64(_) => I64,
        Value::String(_) => STRING,
        Value::Struct(_) => STRUCT,
        Value::Map { .. } => MAP,
        Value::Set { .. } => SET,
        Value::List { .. } => LIST,
    }
}

fn checked_size(value: i32) -> Result<usize, ThriftError> {
    if !(0..=1_000_000).contains(&value) {
        Err(ThriftError::InvalidSize(value))
    } else {
        Ok(value as usize)
    }
}

fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8, std::io::Error> {
    let mut b = [0];
    input.read_exact(&mut b)?;
    Ok(b[0])
}
fn read_i16(input: &mut Cursor<&[u8]>) -> Result<i16, std::io::Error> {
    let mut b = [0; 2];
    input.read_exact(&mut b)?;
    Ok(i16::from_be_bytes(b))
}
fn read_i32(input: &mut Cursor<&[u8]>) -> Result<i32, std::io::Error> {
    let mut b = [0; 4];
    input.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
fn read_i64(input: &mut Cursor<&[u8]>) -> Result<i64, std::io::Error> {
    let mut b = [0; 8];
    input.read_exact(&mut b)?;
    Ok(i64::from_be_bytes(b))
}
fn read_u64(input: &mut Cursor<&[u8]>) -> Result<u64, std::io::Error> {
    let mut b = [0; 8];
    input.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_binary_protocol_round_trips() {
        let fields = vec![Field {
            id: 1,
            value: Value::Struct(vec![Field {
                id: 2,
                value: Value::List {
                    element_type: I32,
                    values: vec![Value::I32(7), Value::I32(8)],
                },
            }]),
        }];
        let raw = write_struct(&fields).unwrap();
        assert_eq!(read_struct(&raw).unwrap(), fields);
        let encoded = encode_transport(&raw).unwrap();
        assert_eq!(decode_transport(&encoded).unwrap(), raw);
    }
}
