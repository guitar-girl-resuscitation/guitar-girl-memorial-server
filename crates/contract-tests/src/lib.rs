#[cfg(test)]
mod tests {
    use ggfm_protocol::{ProtocolSchema, RpcClass};
    #[test]
    fn all_audited_calls_have_an_explicit_behavior_boundary() {
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
}
