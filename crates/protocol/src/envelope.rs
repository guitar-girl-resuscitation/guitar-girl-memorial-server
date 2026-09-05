use std::collections::BTreeMap;

use crate::{
    BOOL, BYTE, DOUBLE, Field, I16, I32, I64, LIST, MAP, ProtocolSchema, RpcSpec, STRING, STRUCT,
    ThriftError, TypeSpec, Value, read_struct, write_struct,
};

#[derive(Clone, Debug)]
pub struct NamedRequest {
    pub call: String,
    pub fields: BTreeMap<String, Value>,
    pub raw_fields: Vec<Field>,
}

impl NamedRequest {
    pub fn value(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn integer(&self, name: &str) -> Option<i64> {
        match self.fields.get(name)? {
            Value::Byte(value) => Some(i64::from(*value)),
            Value::I16(value) => Some(i64::from(*value)),
            Value::I32(value) => Some(i64::from(*value)),
            Value::I64(value) => Some(*value),
            _ => None,
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.fields.get(name)? {
            Value::String(value) => std::str::from_utf8(value).ok(),
            _ => None,
        }
    }

    pub fn integers(&self, name: &str) -> Option<Vec<i64>> {
        let Value::List { values, .. } = self.fields.get(name)? else {
            return None;
        };
        values
            .iter()
            .map(|value| match value {
                Value::Byte(value) => Some(i64::from(*value)),
                Value::I16(value) => Some(i64::from(*value)),
                Value::I32(value) => Some(i64::from(*value)),
                Value::I64(value) => Some(*value),
                _ => None,
            })
            .collect()
    }

    pub fn list(&self, name: &str) -> Option<&[Value]> {
        match self.fields.get(name)? {
            Value::List { values, .. } => Some(values),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolEnvelopeError {
    #[error(transparent)]
    Thrift(#[from] ThriftError),
    #[error("request is missing Call field 1")]
    MissingCall,
    #[error("request is missing Data struct field 2")]
    MissingData,
    #[error("request call {actual} does not match route call {expected}")]
    CallMismatch { expected: String, actual: String },
    #[error("RPC {0} has no wire candidate")]
    MissingWire(String),
    #[error("wire type {0} is not present in the protocol schema")]
    MissingType(String),
    #[error("wire type recursion exceeded the safety limit at {0}")]
    RecursionLimit(String),
    #[error("wire type {0} is not a struct")]
    ExpectedStruct(String),
    #[error("field {field} is not present in wire type {wire_type}")]
    UnknownField { wire_type: String, field: String },
}

pub fn decode_request(
    raw: &[u8],
    expected: &RpcSpec,
    schema: &ProtocolSchema,
) -> Result<NamedRequest, ProtocolEnvelopeError> {
    let envelope = read_struct(raw)?;
    let call = envelope
        .iter()
        .find(|field| field.id == 1)
        .and_then(|field| match &field.value {
            Value::String(value) => String::from_utf8(value.clone()).ok(),
            _ => None,
        })
        .ok_or(ProtocolEnvelopeError::MissingCall)?;
    if call != expected.name {
        return Err(ProtocolEnvelopeError::CallMismatch {
            expected: expected.name.clone(),
            actual: call,
        });
    }
    let raw_fields = envelope
        .into_iter()
        .find(|field| field.id == 2)
        .and_then(|field| match field.value {
            Value::Struct(fields) => Some(fields),
            _ => None,
        })
        .ok_or(ProtocolEnvelopeError::MissingData)?;
    let wire = expected
        .request_wires
        .first()
        .ok_or_else(|| ProtocolEnvelopeError::MissingWire(expected.name.clone()))?;
    let type_spec = resolve_type(schema, &wire.namespace, &wire.wire_type)?;
    let mut fields = BTreeMap::new();
    for field in &type_spec.fields {
        if let Some(value) = raw_fields.iter().find(|value| value.id == field.id) {
            fields.insert(field.name.clone(), value.value.clone());
        }
    }
    Ok(NamedRequest {
        call: expected.name.clone(),
        fields,
        raw_fields,
    })
}

pub fn success_response(
    spec: &RpcSpec,
    schema: &ProtocolSchema,
    unix_seconds: i64,
    data: Option<Value>,
) -> Result<Vec<u8>, ProtocolEnvelopeError> {
    let wire = spec
        .response_wires
        .first()
        .ok_or_else(|| ProtocolEnvelopeError::MissingWire(spec.name.clone()))?;
    let data = match data {
        Some(value) => value,
        None => default_value(schema, &wire.namespace, &wire.wire_type, 0)?,
    };
    response(spec, unix_seconds, 0, "", data, None)
}

pub fn success_response_with_maintenance(
    spec: &RpcSpec,
    schema: &ProtocolSchema,
    unix_seconds: i64,
    data: Option<Value>,
    maintenance: Value,
) -> Result<Vec<u8>, ProtocolEnvelopeError> {
    let wire = spec
        .response_wires
        .first()
        .ok_or_else(|| ProtocolEnvelopeError::MissingWire(spec.name.clone()))?;
    let data = match data {
        Some(value) => value,
        None => default_value(schema, &wire.namespace, &wire.wire_type, 0)?,
    };
    response(spec, unix_seconds, 0, "", data, Some(maintenance))
}

pub fn response_struct(
    spec: &RpcSpec,
    schema: &ProtocolSchema,
    overrides: impl IntoIterator<Item = (String, Value)>,
) -> Result<Value, ProtocolEnvelopeError> {
    let wire = spec
        .response_wires
        .first()
        .ok_or_else(|| ProtocolEnvelopeError::MissingWire(spec.name.clone()))?;
    struct_value(schema, &wire.namespace, &wire.wire_type, overrides)
}

/// Builds a response struct containing only explicitly supplied fields.  Many
/// stock DTOs use absent Thrift fields as nullable values, so emitting a
/// recursively defaulted struct is not equivalent to omitting it.
pub fn response_sparse_struct(
    spec: &RpcSpec,
    schema: &ProtocolSchema,
    fields: impl IntoIterator<Item = (String, Value)>,
) -> Result<Value, ProtocolEnvelopeError> {
    let wire = spec
        .response_wires
        .first()
        .ok_or_else(|| ProtocolEnvelopeError::MissingWire(spec.name.clone()))?;
    sparse_struct_value(schema, &wire.namespace, &wire.wire_type, fields)
}

pub fn sparse_struct_value(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
    fields: impl IntoIterator<Item = (String, Value)>,
) -> Result<Value, ProtocolEnvelopeError> {
    let type_spec = resolve_type(schema, namespace, wire_type)?;
    let mut encoded = Vec::new();
    for (name, value) in fields {
        let field_spec = type_spec
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or(ProtocolEnvelopeError::UnknownField {
                wire_type: wire_type.to_owned(),
                field: name,
            })?;
        encoded.push(Field {
            id: field_spec.id,
            value,
        });
    }
    encoded.sort_by_key(|field| field.id);
    Ok(Value::Struct(encoded))
}

pub fn struct_value(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
    overrides: impl IntoIterator<Item = (String, Value)>,
) -> Result<Value, ProtocolEnvelopeError> {
    let type_spec = resolve_type(schema, namespace, wire_type)?;
    let mut value = default_value(schema, namespace, wire_type, 0)?;
    let Value::Struct(fields) = &mut value else {
        return Err(ProtocolEnvelopeError::ExpectedStruct(wire_type.to_owned()));
    };
    for (name, replacement) in overrides {
        let field_spec = type_spec
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or(ProtocolEnvelopeError::UnknownField {
                wire_type: wire_type.to_owned(),
                field: name,
            })?;
        let field = fields
            .iter_mut()
            .find(|field| field.id == field_spec.id)
            .expect("default struct contains every schema field");
        field.value = replacement;
    }
    Ok(value)
}

pub fn error_response(
    spec: &RpcSpec,
    unix_seconds: i64,
    code: i32,
    message: &str,
) -> Result<Vec<u8>, ProtocolEnvelopeError> {
    response_with_optional_data(spec, unix_seconds, code, message, None, None)
}

fn response(
    spec: &RpcSpec,
    unix_seconds: i64,
    code: i32,
    message: &str,
    data: Value,
    maintenance: Option<Value>,
) -> Result<Vec<u8>, ProtocolEnvelopeError> {
    response_with_optional_data(spec, unix_seconds, code, message, Some(data), maintenance)
}

fn response_with_optional_data(
    spec: &RpcSpec,
    unix_seconds: i64,
    code: i32,
    message: &str,
    data: Option<Value>,
    maintenance: Option<Value>,
) -> Result<Vec<u8>, ProtocolEnvelopeError> {
    let mut fields = vec![Field {
        id: 1,
        value: Value::Struct(vec![
            Field {
                id: 1,
                value: Value::I32(code),
            },
            Field {
                id: 2,
                value: Value::String(message.as_bytes().to_vec()),
            },
        ]),
    }];
    let (mode_id, call_id, data_id, maintenance_id) = if spec.has_server_time {
        fields.push(Field {
            id: 2,
            value: Value::Struct(vec![
                Field {
                    id: 1,
                    value: Value::I32(unix_seconds.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                },
                Field {
                    id: 2,
                    value: Value::I64(unix_seconds),
                },
            ]),
        });
        (3, 4, 5, 6)
    } else {
        (2, 3, 4, 5)
    };
    fields.extend([
        Field {
            id: mode_id,
            value: Value::String(spec.category.as_bytes().to_vec()),
        },
        Field {
            id: call_id,
            value: Value::String(spec.name.as_bytes().to_vec()),
        },
    ]);
    if let Some(data) = data {
        fields.push(Field {
            id: data_id,
            value: data,
        });
    }
    fields.push(Field {
        id: maintenance_id,
        value: maintenance.unwrap_or_else(|| {
            Value::Struct(vec![
                Field {
                    id: 1,
                    value: Value::I16(0),
                },
                Field {
                    id: 2,
                    value: Value::String(Vec::new()),
                },
                Field {
                    id: 3,
                    value: Value::String(Vec::new()),
                },
                Field {
                    id: 4,
                    value: Value::I16(0),
                },
                Field {
                    id: 5,
                    value: Value::String(Vec::new()),
                },
                Field {
                    id: 6,
                    value: Value::String(Vec::new()),
                },
                Field {
                    id: 7,
                    value: Value::String(Vec::new()),
                },
            ])
        }),
    });
    Ok(write_struct(&fields)?)
}

fn default_value(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
    depth: usize,
) -> Result<Value, ProtocolEnvelopeError> {
    if depth > 32 {
        return Err(ProtocolEnvelopeError::RecursionLimit(wire_type.to_owned()));
    }
    if let Some(element) = wire_type.strip_prefix("[]") {
        return Ok(Value::List {
            element_type: wire_kind(schema, namespace, element)?,
            values: Vec::new(),
        });
    }
    if let Some((key, value)) = split_map(wire_type) {
        return Ok(Value::Map {
            key_type: wire_kind(schema, namespace, key)?,
            value_type: wire_kind(schema, namespace, value)?,
            entries: Vec::new(),
        });
    }
    Ok(match wire_type {
        "bool" => Value::Bool(false),
        "byte" | "int8" | "uint8" => Value::Byte(0),
        "int16" | "uint16" => Value::I16(0),
        "int" | "int32" | "uint" | "uint32" => Value::I32(0),
        "int64" | "uint64" => Value::I64(0),
        "float32" | "float64" => Value::Double(0.0),
        "string" => Value::String(Vec::new()),
        _ => {
            let type_spec = resolve_type(schema, namespace, wire_type)?;
            let mut fields = Vec::with_capacity(type_spec.fields.len());
            for field in &type_spec.fields {
                fields.push(Field {
                    id: field.id,
                    value: default_value(
                        schema,
                        &type_spec.namespace,
                        &field.wire_type,
                        depth + 1,
                    )?,
                });
            }
            Value::Struct(fields)
        }
    })
}

fn resolve_type<'a>(
    schema: &'a ProtocolSchema,
    namespace: &str,
    wire_type: &str,
) -> Result<&'a TypeSpec, ProtocolEnvelopeError> {
    let qualified = if let Some((namespace, name)) = wire_type.rsplit_once('.') {
        format!("{namespace}::{name}")
    } else if wire_type.contains("::") {
        wire_type.to_owned()
    } else {
        format!("{namespace}::{wire_type}")
    };
    schema
        .type_spec(&qualified)
        .ok_or(ProtocolEnvelopeError::MissingType(qualified))
}

fn wire_kind(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
) -> Result<u8, ProtocolEnvelopeError> {
    Ok(if wire_type.starts_with("[]") {
        LIST
    } else if wire_type.starts_with("map[") {
        MAP
    } else {
        match wire_type {
            "bool" => BOOL,
            "byte" | "int8" | "uint8" => BYTE,
            "int16" | "uint16" => I16,
            "int" | "int32" | "uint" | "uint32" => I32,
            "int64" | "uint64" => I64,
            "float32" | "float64" => DOUBLE,
            "string" => STRING,
            _ => {
                resolve_type(schema, namespace, wire_type)?;
                STRUCT
            }
        }
    })
}

fn split_map(value: &str) -> Option<(&str, &str)> {
    let remainder = value.strip_prefix("map[")?;
    let close = remainder.find(']')?;
    Some((&remainder[..close], &remainder[close + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_transport, encode_transport};

    #[test]
    fn every_response_has_a_schema_valid_default_envelope() {
        let schema = ProtocolSchema::embedded();
        for rpc in schema.calls() {
            let raw = success_response(rpc, schema, 123, None).unwrap_or_else(|error| {
                panic!("{} cannot create a typed response: {error}", rpc.name)
            });
            assert!(!read_struct(&raw).unwrap().is_empty());
            let encoded = encode_transport(&raw).unwrap();
            assert_eq!(decode_transport(&encoded).unwrap(), raw);
        }
    }

    #[test]
    fn request_envelope_is_named_from_the_schema() {
        let schema = ProtocolSchema::embedded();
        let rpc = schema.call("userLoad").unwrap();
        let raw = write_struct(&[
            Field {
                id: 1,
                value: Value::String(b"userLoad".to_vec()),
            },
            Field {
                id: 2,
                value: Value::Struct(vec![
                    Field {
                        id: 1,
                        value: Value::I32(7),
                    },
                    Field {
                        id: 2,
                        value: Value::String(b"123".to_vec()),
                    },
                ]),
            },
        ])
        .unwrap();
        let request = decode_request(&raw, rpc, schema).unwrap();
        assert_eq!(request.integer("U_seq"), Some(7));
        assert_eq!(request.string("U_id"), Some("123"));
    }

    #[test]
    fn response_fields_are_overridden_by_audited_name() {
        let schema = ProtocolSchema::embedded();
        let rpc = schema.call("buyCheck").unwrap();
        let value = response_struct(
            rpc,
            schema,
            [("Result".to_owned(), Value::String(b"ok".to_vec()))],
        )
        .unwrap();
        let Value::Struct(fields) = value else {
            panic!("buyCheck response must be a struct")
        };
        assert_eq!(fields[0].value, Value::String(b"ok".to_vec()));
    }

    #[test]
    fn post_time_uses_the_production_server_time_wrapper() {
        let schema = ProtocolSchema::embedded();
        let rpc = schema.call("getPostTime").unwrap();
        assert!(rpc.has_server_time);
        assert_eq!(
            rpc.response_candidates,
            ["post_model::GetPostTimeRetDataInfo"]
        );

        let raw = success_response(rpc, schema, 123, None).unwrap();
        let fields = read_struct(&raw).unwrap();
        assert_eq!(
            fields.iter().map(|field| field.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6]
        );
        assert!(matches!(fields[4].value, Value::Struct(_)));
    }

    #[test]
    fn error_response_omits_data_like_the_production_wire() {
        let schema = ProtocolSchema::embedded();
        let rpc = schema.call("userLogin").unwrap();
        let raw = error_response(rpc, 123, 998, "unavailable").unwrap();
        let fields = read_struct(&raw).unwrap();
        assert_eq!(
            fields.iter().map(|field| field.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 6]
        );
    }
}
