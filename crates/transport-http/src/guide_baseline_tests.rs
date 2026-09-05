// Private master input only: no game tables/resources are copied into this test.
// The four-stage synthetic persistence test remains useful for public CI.


fn master_test_state(
    path: std::path::PathBuf,
    master: std::sync::Arc<ggfm_master_data::MasterCatalog>,
) -> AppState {
    AppState {
        database: DatabaseActor::open(path).unwrap(),
        master,
        session: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        capability: std::sync::Arc::from("guide-chain"),
        capability_hash: std::sync::Arc::from(Vec::<u8>::new()),
        endpoint: std::sync::Arc::from("http://127.0.0.1:1"),
        asset_source: None,
        startup_notice_emitted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

async fn guide_test_call(
    state: &AppState,
    session: &LoginSession,
    input: &NamedRequest,
    seq: i64,
    now: i64,
) -> Value {
    execute(
        state,
        session,
        GameplayCall {
            rpc: RpcName::SetUserFollowerQuest,
            locale: "en",
            request: input,
            clock: DeviceClock::new(now + seq, 600).unwrap(),
            sequence: Some(seq),
            raw_request: format!("guide:{now}:{seq}").as_bytes(),
        },
    )
    .await
    .unwrap()
    .unwrap()
}
