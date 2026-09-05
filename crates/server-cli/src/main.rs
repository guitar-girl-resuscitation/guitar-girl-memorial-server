use ggfm_domain::DeviceClock;
use ggfm_transport_http::ServerConfig;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|error| std::io::Error::other(error.to_string()))?;
    let capability = std::env::var("GGFM_CAPABILITY").unwrap_or_else(|_| hex::encode(random));
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let master_data_path = std::env::var_os("GGFM_MASTER_SQLITE")
        .map(PathBuf::from)
        .ok_or("GGFM_MASTER_SQLITE must point to a Patcher-generated master.sqlite")?;
    let database_path = std::env::var_os("GGFM_DATABASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ggfm.sqlite3"));
    let mut server = ggfm_transport_http::start(ServerConfig {
        database_path,
        master_data_path,
        capability,
        clock: DeviceClock::new(now, 0)?,
        asset_source: None,
        progress: None,
    })
    .await?;
    server.begin_login(DeviceClock::new(now, 0)?).await?;
    println!("{}", server.endpoint);
    tokio::signal::ctrl_c().await?;
    server.shutdown();
    Ok(())
}
