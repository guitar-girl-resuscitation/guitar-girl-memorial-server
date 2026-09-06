// The original master stays private. The tested Rust crate is a development
// oracle, not a production dependency or a source of shipped game resources.

#[test]
fn ch3_wire_reports_completed_history_not_the_unplayed_frontier() {
    let row = |id, completed, stars| ggfm_domain::Ch3StageSnapshot {
        stage_id: id, chapter: 1, stage_index: (id - 1000) as i32,
        completed, best_star: stars, best_score: 0,
    };
    assert!(ch3_completed_stage_values(&[row(1001, false, 0)]).unwrap().is_empty());
    let first_clear = ch3_completed_stage_values(&[row(1001, true, 0), row(1002, false, 0)]).unwrap();
    assert_eq!(first_clear, vec![ch3_stage_wire_value(&row(1001, true, 0)).unwrap()]);
    let second_clear = ch3_completed_stage_values(&[row(1001, true, 0), row(1002, true, 1), row(1003, false, 0)]).unwrap();
    assert_eq!(second_clear.len(), 2);
    assert_eq!(nested_field(&second_clear[1], 1), Some(&Value::I32(1002)));
}


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
