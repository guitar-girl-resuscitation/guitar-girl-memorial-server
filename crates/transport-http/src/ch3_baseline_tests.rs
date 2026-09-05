// The original master stays private. The tested Rust crate is a development
// oracle, not a production dependency or a source of shipped game resources.


#[test]
fn ch3_energy_projects_an_absolute_full_deadline_not_remaining_seconds() {
    for (amount, expected) in [
        (100, 1_788_360_001),
        (95, 1_788_360_005),
        (0, 1_788_360_100),
    ] {
        let value = ch3_energy_wire_value(&ggfm_domain::Ch3EnergySnapshot {
            amount,
            cap: 100,
            calculated_at: 1_788_360_000,
        })
        .unwrap();
        assert_eq!(nested_field(&value, 1), Some(&Value::I32(amount)));
        assert_eq!(nested_field(&value, 2), Some(&Value::I32(expected)));
        assert_eq!(nested_field(&value, 3), Some(&Value::I32(100)));
    }
}
