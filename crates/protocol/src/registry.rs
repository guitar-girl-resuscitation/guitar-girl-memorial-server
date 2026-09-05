use std::{collections::BTreeMap, sync::OnceLock};

use serde::Deserialize;

#[cfg(test)]
const ACTIVE_CALLS: &str = include_str!("../schema/active_calls.txt");
#[cfg(test)]
const TRANSPORT_ONLY_CALLS: &str = include_str!("../schema/transport_only_calls.txt");
#[cfg(test)]
const DTO_ONLY_CALLS: &str = include_str!("../schema/dto_only_calls.txt");
const SCHEMA_JSON: &str = include_str!("../schema/protocol.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcClass {
    Active,
    TransportOnly,
    DtoOnly,
}

include!(concat!(env!("OUT_DIR"), "/rpc_names.rs"));

#[derive(Clone, Debug, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub wire_type: String,
    pub id: i16,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TypeSpec {
    pub namespace: String,
    pub name: String,
    pub fields: Vec<FieldSpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RpcSpec {
    pub name: String,
    pub category: String,
    pub class: String,
    pub request_candidates: Vec<String>,
    pub response_candidates: Vec<String>,
    pub request_wires: Vec<WireSpec>,
    pub response_wires: Vec<WireSpec>,
    pub has_server_time: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireSpec {
    pub namespace: String,
    pub wire_type: String,
}

impl RpcSpec {
    pub fn rpc_class(&self) -> RpcClass {
        match self.class.as_str() {
            "active" => RpcClass::Active,
            "transport-only" => RpcClass::TransportOnly,
            "dto-only" => RpcClass::DtoOnly,
            other => panic!("unknown RPC class {other}"),
        }
    }

    pub fn rpc_name(&self) -> RpcName {
        RpcName::from_wire_name(&self.name).expect("schema RPC is present in generated registry")
    }
}

#[derive(Debug, Deserialize)]
struct ProtocolDocument {
    calls: Vec<RpcSpec>,
    types: Vec<TypeSpec>,
}

#[derive(Debug)]
pub struct ProtocolSchema {
    calls: BTreeMap<String, RpcSpec>,
    types: BTreeMap<String, TypeSpec>,
}

impl ProtocolSchema {
    pub fn embedded() -> &'static Self {
        static INSTANCE: OnceLock<ProtocolSchema> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let document: ProtocolDocument =
                serde_json::from_str(SCHEMA_JSON).expect("embedded protocol schema is valid");
            ProtocolSchema {
                calls: document
                    .calls
                    .into_iter()
                    .map(|spec| (spec.name.clone(), spec))
                    .collect(),
                types: document
                    .types
                    .into_iter()
                    .map(|spec| (format!("{}::{}", spec.namespace, spec.name), spec))
                    .collect(),
            }
        })
    }

    pub fn call(&self, name: &str) -> Option<&RpcSpec> {
        self.calls.get(name)
    }

    pub fn call_typed(&self, name: RpcName) -> &RpcSpec {
        self.calls
            .get(name.as_str())
            .expect("generated RPC is present in embedded schema")
    }

    pub fn type_spec(&self, name: &str) -> Option<&TypeSpec> {
        self.types.get(name)
    }

    pub fn calls(&self) -> impl Iterator<Item = &RpcSpec> {
        self.calls.values()
    }

    pub fn types(&self) -> impl Iterator<Item = &TypeSpec> {
        self.types.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(value: &str) -> usize {
        value.lines().filter(|line| !line.trim().is_empty()).count()
    }

    #[test]
    fn audited_classification_is_complete_and_disjoint() {
        assert_eq!(lines(ACTIVE_CALLS), 69);
        assert_eq!(lines(TRANSPORT_ONLY_CALLS), 45);
        assert_eq!(lines(DTO_ONLY_CALLS), 13);
        let schema = ProtocolSchema::embedded();
        assert_eq!(schema.calls().count(), 127);
        assert_eq!(
            schema
                .calls()
                .filter(|c| c.rpc_class() == RpcClass::Active)
                .count(),
            69
        );
        assert_eq!(
            schema
                .calls()
                .filter(|c| c.rpc_class() == RpcClass::TransportOnly)
                .count(),
            45
        );
        assert_eq!(
            schema
                .calls()
                .filter(|c| c.rpc_class() == RpcClass::DtoOnly)
                .count(),
            13
        );
    }

    #[test]
    fn every_wire_candidate_resolves() {
        let schema = ProtocolSchema::embedded();
        for call in schema.calls() {
            for name in call
                .request_candidates
                .iter()
                .chain(&call.response_candidates)
            {
                assert!(
                    schema.type_spec(name).is_some(),
                    "missing {name} for {}",
                    call.name
                );
            }
        }
    }

    #[test]
    fn generated_registry_exactly_matches_schema() {
        let schema = ProtocolSchema::embedded();
        assert_eq!(RpcName::ALL.len(), 127);
        for rpc in RpcName::ALL {
            let spec = schema.call_typed(rpc);
            assert_eq!(spec.name, rpc.as_str());
            assert_eq!(spec.category, rpc.category());
            assert_eq!(spec.rpc_class(), rpc.class());
        }
    }

    #[test]
    fn production_verified_subscription_fields_do_not_regress_to_declaration_order() {
        let schema = ProtocolSchema::embedded();
        let subscribe = schema
            .type_spec("main_model::GetSubscribeList")
            .expect("subscription master DTO");
        let fields = subscribe
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.wire_type.as_str(), field.id))
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            [
                ("I_id", "int64", 1),
                ("I_Area", "int16", 2),
                ("S_Type", "string", 3),
                ("I_TimeLimit", "int16", 4),
                ("B_IsActive", "int16", 7),
                ("I_StartYear", "int32", 11),
                ("I_RepeatMonth", "int32", 12),
                ("I_MonthGroupIndex", "int32", 13),
            ]
        );
    }
}
