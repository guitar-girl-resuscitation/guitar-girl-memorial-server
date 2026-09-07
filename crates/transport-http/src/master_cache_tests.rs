use super::*;

#[test]
fn same_day_upgrade_invalidates_old_fractional_price_cache() {
    for offset in [-720, -210, 0, 330, 600, 840] {
        let clock = DeviceClock::new(1_800_000_000, offset).unwrap();
        let midnight = clock.local_epoch_day() * 86_400 - i64::from(offset) * 60;
        for revision in [0, 1, 20] {
            let previous_generation = midnight + 10 + revision;
            assert!(master_update_time(clock, revision) > previous_generation);
            assert_eq!(master_update_time(clock, revision),
                       midnight + MASTER_OVERLAY_GENERATION + revision);
        }
    }
}

#[test]
fn restart_is_stable_and_next_local_day_refreshes() {
    for offset in [-720, -210, 0, 330, 600, 840] {
        let midnight = 20_000 * 86_400 - i64::from(offset) * 60;
        let morning = DeviceClock::new(midnight + 60, offset).unwrap();
        let evening = DeviceClock::new(midnight + 86_399, offset).unwrap();
        let tomorrow = DeviceClock::new(midnight + 86_400, offset).unwrap();
        assert_eq!(master_update_time(morning, 3), master_update_time(evening, 3));
        assert_eq!(master_update_time(tomorrow, 3) - master_update_time(evening, 3), 86_400);
        assert_eq!(master_update_time(evening, 4) - master_update_time(evening, 3), 1);
    }
}
