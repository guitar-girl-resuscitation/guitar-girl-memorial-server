mod gameplay;
mod mail_copy;
mod handlers;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use ggfm_domain::{ContentKind, CurrencyKind, DeviceClock, LoginSession, Reward, UserIdentity};
use ggfm_master_data::{
    MasterCatalog, MasterData, MasterRow, MasterScalar, RewardGroupRow, ShopRow,
};
use ggfm_persistence_sqlite::{DatabaseActor, RpcMutation, StoreError};
use ggfm_protocol::{
    ProtocolSchema, RpcName, Value, decode_request, decode_transport, encode_transport,
    error_response, sparse_struct_value, success_response, success_response_with_maintenance,
};
use handlers::handler_for;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
};

pub const SESSION_HEADER: &str = "x-ggfm-session";
pub const REQUEST_SEQ_HEADER: &str = "x-ggfm-request-seq";
pub const DEVICE_TIME_HEADER: &str = "x-ggfm-device-time";
pub const UTC_OFFSET_HEADER: &str = "x-ggfm-utc-offset";

/// Read-only access to assets which remain inside the user's verified game
/// package. Implementations must treat `name` as an APK asset name, never as a
/// host filesystem path.
pub trait AssetSource: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String>;

    /// Return the packaged asset length without loading its contents. Android
    /// clients probe bundles with HEAD before downloading them, and some of
    /// those bundles are large enough that reading them here would be wasteful.
    fn len(&self, name: &str) -> Result<Option<u64>, String> {
        self.read(name)
            .map(|bytes| bytes.map(|bytes| bytes.len() as u64))
    }
}

#[cfg(target_os = "android")]
fn runtime_log(message: &str) {
    use std::{
        ffi::{CString, c_char},
        os::raw::c_int,
    };

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    let Ok(text) = CString::new(message.replace('\0', "?")) else {
        return;
    };
    // ANDROID_LOG_INFO = 4. The tag is a static NUL-terminated C string.
    unsafe {
        __android_log_write(4, c"GGFM-SERVER".as_ptr(), text.as_ptr());
    }
}

#[cfg(not(target_os = "android"))]
fn runtime_log(message: &str) {
    tracing::info!(message, "embedded memorial transport");
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) database: DatabaseActor,
    pub(crate) master: Arc<MasterCatalog>,
    pub(crate) session: Arc<Mutex<Option<LoginSession>>>,
    pub(crate) capability: Arc<str>,
    pub(crate) capability_hash: Arc<[u8]>,
    pub(crate) endpoint: Arc<str>,
    pub(crate) asset_source: Option<Arc<dyn AssetSource>>,
    /// The stock startup flow can request update time more than once while it
    /// refreshes a missing master bundle.  The memorial disclosure belongs to
    /// the cold process, not to every request, so only the first response in a
    /// server lifetime carries MaintenanceData.
    pub(crate) startup_notice_emitted: Arc<AtomicBool>,
}

pub struct ServerHandle {
    pub endpoint: String,
    pub capability: String,
    pub session: Arc<Mutex<Option<LoginSession>>>,
    state: AppState,
    database_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    progress: Option<StartupProgressSink>,
}

impl ServerHandle {
    pub fn shutdown(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
    }

    /// Begin a cold-login session and atomically activate a pending slot.
    pub async fn begin_login(&self, clock: DeviceClock) -> Result<LoginSession, String> {
        self.report_startup("[INFO] identity: activating selected slot and creating local session");
        let capability_hash = self.state.capability_hash.to_vec();
        let mut session = self
            .state
            .database
            .execute(move |database| database.begin_login(&capability_hash, clock))
            .await
            .map_err(|error| {
                self.report_startup(&format!("[ERROR] identity: session activation failed: {error}"));
                error.to_string()
            })?;
        session.capability = self.capability.clone();
        *self.state.session.lock().await = Some(session.clone());
        self.report_startup("[INFO] identity: slot activated; isolated local session ready");
        Ok(session)
    }

    fn report_startup(&self, message: &str) {
        runtime_log(message);
        if let Some(progress) = &self.progress { progress(message); }
    }

    pub async fn prepare_shutdown(&self, now: i64) -> Result<(), String> {
        if self.state.session.lock().await.is_none() {
            return Ok(());
        }
        self.state
            .database
            .execute(|database| database.checkpoint())
            .await
            .map_err(|error| error.to_string())?;
        let temporary = self.database_path.with_extension(format!(
            "sqlite3.backup.{}.{}.new",
            std::process::id(),
            now
        ));
        let destination = self.database_path.with_extension("sqlite3.backup");
        let backup_path = temporary.clone();
        self.state
            .database
            .execute(move |database| database.backup_to(&backup_path))
            .await
            .map_err(|error| error.to_string())?;
        publish_backup(&temporary, &destination).map_err(|error| error.to_string())
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub database_path: PathBuf,
    pub master_data_path: PathBuf,
    pub capability: String,
    pub clock: DeviceClock,
    pub asset_source: Option<Arc<dyn AssetSource>>,
    /// Optional bounded startup event sink used by the Android diagnostic gate.
    pub progress: Option<StartupProgressSink>,
}

pub type StartupProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

fn startup_progress(config: &ServerConfig, message: &str) {
    runtime_log(message);
    if let Some(progress) = &config.progress {
        progress(message);
    }
}

fn publish_backup(temporary: &FsPath, destination: &FsPath) -> std::io::Result<()> {
    let previous = destination.with_extension("backup.previous");
    if previous.exists() {
        fs::remove_file(&previous)?;
    }
    let had_current = destination.exists();
    if had_current {
        fs::rename(destination, &previous)?;
    }
    if let Err(error) = fs::rename(temporary, destination) {
        if had_current {
            let _ = fs::rename(&previous, destination);
        }
        return Err(error);
    }
    Ok(())
}

fn sidecar_path(database: &FsPath, suffix: &str) -> PathBuf {
    let mut value = database.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn open_database_with_recovery(path: &FsPath, now: i64) -> Result<DatabaseActor, String> {
    let first_error = match DatabaseActor::open(path.to_owned()) {
        Ok(database) => return Ok(database),
        Err(error) => error.to_string(),
    };
    if !path.exists() {
        return Err(first_error);
    }
    let backup = path.with_extension("sqlite3.backup");
    let previous = backup.with_extension("backup.previous");
    let candidates = [backup, previous];
    if !candidates.iter().any(|candidate| candidate.exists()) {
        return Err(first_error);
    }

    let mut preserved = Vec::new();
    for original in [
        path.to_owned(),
        sidecar_path(path, "-wal"),
        sidecar_path(path, "-shm"),
    ] {
        if original.exists() {
            let mut archived = original.as_os_str().to_os_string();
            archived.push(format!(".corrupt.{now}"));
            let archived = PathBuf::from(archived);
            fs::rename(&original, &archived).map_err(|error| error.to_string())?;
            preserved.push((original, archived));
        }
    }

    let mut recovery_errors = Vec::new();
    for candidate in candidates.iter().filter(|candidate| candidate.exists()) {
        if path.exists() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        fs::copy(candidate, path).map_err(|error| error.to_string())?;
        match DatabaseActor::open(path.to_owned()) {
            Ok(database) => return Ok(database),
            Err(error) => recovery_errors.push(format!("{}: {error}", candidate.display())),
        }
    }

    if path.exists() {
        let _ = fs::remove_file(path);
    }
    for (original, archived) in preserved.into_iter().rev() {
        let _ = fs::rename(archived, original);
    }
    Err(format!(
        "primary database failed ({first_error}); backups failed ({})",
        recovery_errors.join("; ")
    ))
}

pub async fn start(config: ServerConfig) -> Result<ServerHandle, String> {
    startup_progress(&config, "[INFO] master: opening read-only catalog; validating integrity and required tables");
    let mut master = MasterData::open_read_only(&config.master_data_path)
        .and_then(|master| master.load_catalog())
        .map_err(|error| {
            startup_progress(&config, &format!("[ERROR] master: catalog loading failed: {error}"));
            // Preserve SQLite's extended error code and filesystem evidence.
            // Do not delete or recreate player saves to mask catalog failures.
            startup_progress(&config, &format!("[ERROR] master: detail={error:?}"));
            match fs::metadata(&config.master_data_path) {
                Ok(metadata) => startup_progress(&config, &format!(
                    "[INFO] master: file={} bytes={} readonly={}",
                    metadata.is_file(), metadata.len(), metadata.permissions().readonly())),
                Err(io_error) => startup_progress(&config, &format!(
                    "[ERROR] master: stat failed: {io_error:?}")),
            }
            error.to_string()
        })?;
    startup_progress(&config, &format!("[INFO] master: loaded schema={} source_tables={} costumes={} guitars={} music={} skills={}",
        master.metadata.version, master.metadata.source_table_count, master.costumes.len(),
        master.guitars.len(), master.music.len(), master.skills.len()));
    startup_progress(&config, "[INFO] policy: applying shared memorial shop rules");
    apply_memorial_shop_policy(&mut master).map_err(|error| {
        startup_progress(&config, &format!("[ERROR] policy: shop projection failed: {error}"));
        error
    })?;
    startup_progress(&config, "[INFO] policy: shop projection complete");
    startup_progress(&config, "[INFO] storage: opening player database and checking schema/integrity");
    let database = open_database_with_recovery(&config.database_path, config.clock.unix_seconds)
        .map_err(|error| {
            startup_progress(&config, &format!("[ERROR] storage: database open failed: {error}"));
            error
        })?;
    let settings = database.execute(|db| db.connection_diagnostics()).await.map_err(|error| error.to_string())?;
    startup_progress(&config, &format!("[INFO] storage: database opened; integrity check passed; {settings}"));
    let reward_rules = ggfm_domain::RewardRules {
        costume_fan_bonuses: master.costumes.iter().map(|row| {
            (row.id, (row.area, memorial_costume_fan_bonus(&master, row) as f32))
        }).collect(),
    };
    database.execute(move |db| db.configure_reward_rules(reward_rules)).await.map_err(|error| error.to_string())?;
    startup_progress(&config, "[INFO] rewards: shared costume bonus catalog ready; original fan reward bonuses retained");
    let initial_time = config.clock.unix_seconds;
    let achievement_ids: Vec<i64> = master.achievements.iter().filter(|row| row.active).map(|row| row.id).collect();
    let achievement_count = achievement_ids.len();
    let inserted = database.execute(move |db| db.configure_achievement_catalog(achievement_ids, initial_time))
        .await.map_err(|error| error.to_string())?;
    startup_progress(&config, &format!("[INFO] achievements: catalog={achievement_count} missing_records_added={inserted}; existing progress and claims preserved"));
    database
        .execute(move |database| database.ensure_default_slot(initial_time))
        .await
        .map_err(|error| error.to_string())?;
    startup_progress(&config, "[INFO] identity: save slot available");
    let capability_hash: Arc<[u8]> =
        Arc::from(Sha256::digest(config.capability.as_bytes()).to_vec());
    let session = Arc::new(Mutex::new(None));
    startup_progress(&config, "[INFO] transport: binding random IPv4 loopback port");
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|error| {
            startup_progress(&config, &format!("[ERROR] transport: local bind failed: {error}"));
            error.to_string()
        })?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let endpoint: Arc<str> = Arc::from(format!("http://{address}"));
    let state = AppState {
        database,
        master: Arc::new(master),
        session: session.clone(),
        capability: Arc::from(config.capability.clone()),
        capability_hash,
        endpoint: endpoint.clone(),
        asset_source: config.asset_source.clone(),
        startup_notice_emitted: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/memorial/notice/{locale}", get(memorial_notice))
        .route("/memorial/v1/session", get(current_session))
        .route("/memorial/v1/slots", get(list_slots).post(create_slot))
        .route(
            "/memorial/v1/slots/{usn}",
            axum::routing::delete(delete_slot),
        )
        .route("/memorial/v1/slots/{usn}/select", post(select_slot))
        .route("/memorial/v1/legacy-items", get(list_legacy_items))
        .route(
            "/memorial/v1/legacy-items/{kind}/{area}/{id}/mail",
            post(send_legacy_item),
        )
        .route(
            "/memorial/v1/currency/{kind}/mail",
            post(send_currency_mail),
        )
        .route("/memorial/v1/star-pass", get(list_star_passes))
        .route(
            "/memorial/v1/star-pass/{season}/select",
            post(select_star_pass),
        )
        .route("/{category}/{rpc}/{locale}/", post(legacy_rpc))
        .fallback(packaged_asset)
        .layer(middleware::from_fn(log_request_head))
        .with_state(state.clone());
    let (shutdown, receiver) = oneshot::channel();
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
        {
            tracing::error!(%error, "embedded memorial server stopped unexpectedly");
        }
    });
    startup_progress(&config, &format!("[INFO] transport: listening at {endpoint}; session authentication required"));
    Ok(ServerHandle {
        endpoint: endpoint.to_string(),
        capability: config.capability,
        session,
        state,
        database_path: config.database_path,
        shutdown: Some(shutdown),
        progress: config.progress,
    })
}

pub(crate) fn memorial_costume_fan_bonus(master: &MasterCatalog, row: &ggfm_master_data::CostumeRow) -> f64 {
    let is_default = master.costumes.iter().filter(|c| c.area == row.area).map(|c| c.id).min() == Some(row.id);
    let category_max = master.costumes.iter().filter(|c| c.area == row.area).map(|c| c.fan_multiplier).fold(0.0, f64::max);
    ggfm_policy_memorial::costume_fan_bonus(row.area, is_default, row.fan_multiplier, category_max)
}

fn apply_memorial_shop_policy(master: &mut MasterCatalog) -> Result<(), String> {
    let policy = ggfm_policy_memorial::MemorialPolicy::default();
    let shop_table = master
        .game_data
        .get("Shop")
        .ok_or_else(|| "master projection has no Shop table".to_owned())?;
    let reward_table = master
        .game_data
        .get("Reward_group")
        .ok_or_else(|| "master projection has no Reward_group table".to_owned())?;

    let policy_shop_ids = policy
        .memorial_shop
        .items
        .iter()
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    let policy_reward_groups = policy
        .memorial_shop
        .items
        .iter()
        .map(|item| item.reward_group)
        .collect::<BTreeSet<_>>();
    let existing_shop_count = master
        .shops
        .iter()
        .filter(|row| policy_shop_ids.contains(&row.id))
        .count();
    let existing_reward_count = master
        .reward_groups
        .iter()
        .filter(|row| policy_reward_groups.contains(&row.group))
        .count();
    if existing_shop_count != 0 || existing_reward_count != 0 {
        let expected_reward_count = policy
            .memorial_shop
            .items
            .iter()
            .map(|item| item.rewards.len())
            .sum::<usize>();
        if existing_shop_count != policy.memorial_shop.items.len()
            || existing_reward_count != expected_reward_count
        {
            return Err(format!(
                "partial memorial shop overlay: {existing_shop_count}/{} shops and \
                 {existing_reward_count}/{expected_reward_count} rewards",
                policy.memorial_shop.items.len()
            ));
        }
        for item in &policy.memorial_shop.items {
            let shop = master
                .shops
                .iter()
                .find(|row| row.id == item.id)
                .ok_or_else(|| format!("missing memorial shop item {}", item.id))?;
            if shop.product_type != policy.memorial_shop.product_type as i32
                || shop.reward_group != item.reward_group
                || shop.sell_type != policy.memorial_shop.sell_type as i32
                || shop.sell_value != policy.memorial_shop.sell_value
                || !shop.active
            {
                return Err(format!(
                    "memorial shop item {} does not match the policy manifest",
                    item.id
                ));
            }
            let expected = item
                .rewards
                .iter()
                .map(|reward| (reward.reward_type, reward.id, reward.quantity))
                .collect::<Vec<_>>();
            let actual = master
                .reward_groups
                .iter()
                .filter(|row| row.group == item.reward_group)
                .map(|row| (row.reward_type, row.reward_id, row.quantity))
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(format!(
                    "memorial reward group {} does not match the policy manifest",
                    item.reward_group
                ));
            }
            if !shop_table
                .iter()
                .any(|row| master_row_i64(row, "id") == Some(item.id))
                || reward_table
                    .iter()
                    .filter(|row| master_row_i64(row, "group") == Some(item.reward_group))
                    .count()
                    != item.rewards.len()
            {
                return Err(format!(
                    "memorial shop projection is incomplete for item {}",
                    item.id
                ));
            }
        }
        return Ok(());
    }

    for item in &policy.memorial_shop.items {
        if master.shops.iter().any(|row| row.id == item.id)
            || shop_table
                .iter()
                .any(|row| master_row_i64(row, "id") == Some(item.id))
        {
            return Err(format!(
                "memorial shop id {} collides with source master",
                item.id
            ));
        }
        if master
            .reward_groups
            .iter()
            .any(|row| row.group == item.reward_group)
            || reward_table
                .iter()
                .any(|row| master_row_i64(row, "group") == Some(item.reward_group))
        {
            return Err(format!(
                "memorial reward group {} collides with source master",
                item.reward_group
            ));
        }
    }

    for item in &policy.memorial_shop.items {
        master.shops.push(ShopRow {
            id: item.id,
            product_type: policy.memorial_shop.product_type as i32,
            reward_group: item.reward_group,
            sell_type: policy.memorial_shop.sell_type as i32,
            sell_value: policy.memorial_shop.sell_value,
            active: true,
            condition: 0,
            condition_value: 0,
        });
        master
            .game_data
            .get_mut("Shop")
            .expect("Shop table was validated")
            .push(memorial_shop_master_row(&policy.memorial_shop, item));

        for (index, reward) in item.rewards.iter().enumerate() {
            let row_id = item
                .reward_group
                .checked_mul(100)
                .and_then(|base| base.checked_add(index as i64 + 1))
                .ok_or_else(|| format!("memorial reward row id overflow for item {}", item.id))?;
            master.reward_groups.push(RewardGroupRow {
                row_id,
                group: item.reward_group,
                reward_type: reward.reward_type,
                reward_id: reward.id,
                quantity: reward.quantity,
                first_purchase_quantity: 0.0,
            });
            master
                .game_data
                .get_mut("Reward_group")
                .expect("Reward_group table was validated")
                .push(BTreeMap::from([
                    ("id".to_owned(), MasterScalar::Integer(row_id)),
                    ("group".to_owned(), MasterScalar::Integer(item.reward_group)),
                    (
                        "rewardtype".to_owned(),
                        MasterScalar::Integer(i64::from(reward.reward_type)),
                    ),
                    (
                        "rewardid".to_owned(),
                        MasterScalar::Integer(i64::from(reward.id)),
                    ),
                    (
                        "rewardquantity".to_owned(),
                        MasterScalar::Real(reward.quantity),
                    ),
                    ("buyfirstquantity".to_owned(), MasterScalar::Integer(0)),
                ]));
        }
    }
    master.shops.sort_by_key(|row| row.id);
    master
        .reward_groups
        .sort_by_key(|row| (row.group, row.row_id));
    Ok(())
}

fn localized_or_english(values: &BTreeMap<String, String>, locale: &str) -> String {
    values
        .get(locale)
        .or_else(|| values.get("en"))
        .cloned()
        .unwrap_or_default()
}

fn memorial_shop_master_row(
    shop: &ggfm_policy_memorial::MemorialShopPolicy,
    item: &ggfm_policy_memorial::MemorialShopItem,
) -> MasterRow {
    let mut row = MasterRow::from([
        ("id".to_owned(), MasterScalar::Integer(item.id)),
        (
            "productidios".to_owned(),
            MasterScalar::Text(item.product_id_ios.clone()),
        ),
        (
            "productidaos".to_owned(),
            MasterScalar::Text(item.product_id_android.clone()),
        ),
        (
            "shopcategory".to_owned(),
            MasterScalar::Integer(shop.shop_category),
        ),
        (
            "producttype".to_owned(),
            MasterScalar::Integer(shop.product_type),
        ),
        (
            "resourcename".to_owned(),
            MasterScalar::Text(item.resource_name.clone()),
        ),
        (
            "rewardgroup".to_owned(),
            MasterScalar::Integer(item.reward_group),
        ),
        ("tag".to_owned(), MasterScalar::Integer(0)),
        ("selltype".to_owned(), MasterScalar::Integer(shop.sell_type)),
        (
            "sellvalue".to_owned(),
            MasterScalar::Integer(shop.sell_value),
        ),
        ("isactive".to_owned(), MasterScalar::Integer(1)),
        ("condition".to_owned(), MasterScalar::Integer(0)),
        ("conditionvalue".to_owned(), MasterScalar::Integer(0)),
        (
            "storepriceaos".to_owned(),
            MasterScalar::Text("$0.00".into()),
        ),
        (
            "storepriceios".to_owned(),
            MasterScalar::Text("$0.00".into()),
        ),
        (
            "korstorepriceaos".to_owned(),
            MasterScalar::Text("$0.00".into()),
        ),
        (
            "korstorepriceios".to_owned(),
            MasterScalar::Text("$0.00".into()),
        ),
        (
            "sortindex".to_owned(),
            MasterScalar::Integer(i64::from(item.sort_index)),
        ),
        (
            "arealist".to_owned(),
            MasterScalar::Text(shop.area_list.clone()),
        ),
        (
            "altresourcename".to_owned(),
            MasterScalar::Text(String::new()),
        ),
        ("islimittime".to_owned(), MasterScalar::Integer(0)),
        ("uistarttime".to_owned(), MasterScalar::Text(String::new())),
        ("starttime".to_owned(), MasterScalar::Text(String::new())),
        ("endtime".to_owned(), MasterScalar::Text(String::new())),
    ]);
    for (wire_suffix, policy_key) in [
        ("ko", "ko"),
        ("en", "en"),
        ("ja", "ja"),
        ("zhchs", "zhChs"),
        ("zhcht", "zhCht"),
        ("vi", "vi"),
        ("es", "es"),
        ("it", "it"),
        ("id", "id"),
        ("th", "th"),
        ("pt", "pt"),
        ("hi", "hi"),
    ] {
        let title = localized_or_english(&item.title, policy_key);
        let description = localized_or_english(&item.description, policy_key);
        row.insert(format!("title{wire_suffix}"), MasterScalar::Text(title));
        row.insert(
            format!("description{wire_suffix}"),
            MasterScalar::Text(description.clone()),
        );
        row.insert(
            format!("altdescription{wire_suffix}"),
            MasterScalar::Text(description),
        );
    }
    row
}

async fn packaged_asset(State(state): State<AppState>, request: Request) -> Response {
    let Some(is_head) = packaged_asset_is_head(request.method()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(name) = normalize_asset_name(request.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(source) = state.asset_source.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let read_name = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        if is_head {
            source
                .len(&read_name)
                .map(|length| length.map(AssetReply::Length))
        } else {
            source
                .read(&read_name)
                .map(|bytes| bytes.map(AssetReply::Bytes))
        }
    })
    .await;
    match result {
        Ok(Ok(Some(AssetReply::Length(length)))) => {
            runtime_log(&format!(
                "packaged asset probe: name={name} response_bytes={length}"
            ));
            Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
                .header(axum::http::header::CONTENT_LENGTH, length)
                .header(
                    axum::http::header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable",
                )
                .body(Body::empty())
                .unwrap_or_else(|error| internal_error(error.to_string()))
        }
        Ok(Ok(Some(AssetReply::Bytes(bytes)))) => {
            runtime_log(&format!(
                "packaged asset: name={name} response_bytes={}",
                bytes.len()
            ));
            Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
                .header(axum::http::header::CONTENT_LENGTH, bytes.len())
                .header(
                    axum::http::header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable",
                )
                .body(Body::from(bytes))
                .unwrap_or_else(|error| internal_error(error.to_string()))
        }
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(error)) => internal_error(error),
        Err(error) => internal_error(error),
    }
}

enum AssetReply {
    Length(u64),
    Bytes(Vec<u8>),
}

fn packaged_asset_is_head(method: &Method) -> Option<bool> {
    match *method {
        Method::GET => Some(false),
        Method::HEAD => Some(true),
        _ => None,
    }
}

fn normalize_asset_name(path: &str) -> Option<String> {
    let mut components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    while components.len() >= 2
        && components[0] == "AssetBundles"
        && components[1] == "AssetBundles"
    {
        components.remove(0);
    }
    if components.first().copied() != Some("AssetBundles")
        || components.len() < 2
        || components.iter().any(|component| {
            *component == "." || *component == ".." || component.contains(['\\', '%', ':', '\0'])
        })
    {
        return None;
    }
    Some(components.join("/"))
}

async fn log_request_head(request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let content_length = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<none>");
    let transfer_encoding = request
        .headers()
        .get(axum::http::header::TRANSFER_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<none>");
    let has_session = request.headers().contains_key(SESSION_HEADER);
    runtime_log(&format!(
        "request head: {method} {path} content-length={content_length} transfer-encoding={transfer_encoding} session_header={has_session}"
    ));
    next.run(request).await
}

async fn memorial_notice(Path(locale): Path<String>) -> Html<String> {
    let locale = normalized_notice_locale(&locale);
    let (title, body) = memorial_notice_copy(locale);
    Html(format!(
        "<!doctype html><html lang='{locale}'><meta charset=utf-8><meta name=viewport content='width=device-width'><title>{title}</title><style>body{{font-family:sans-serif;background:#fff6f8;color:#542b3a;max-width:42rem;margin:3rem auto;padding:0 1.5rem;line-height:1.7}}h1{{color:#e64f79}}</style><h1>{title}</h1><p>{body}</p></html>"
    ))
}

pub(crate) fn normalized_notice_locale(locale: &str) -> &'static str {
    let locale = locale.trim().to_ascii_lowercase().replace('_', "-");
    match locale.as_str() {
        value if value == "ko" || value.starts_with("ko-") || value == "kr" => "ko",
        value if value == "ja" || value.starts_with("ja-") || value == "jp" => "ja",
        "zh-hans" | "zh-chs" | "zh-cn" | "zh-sg" | "chs" | "cn" => "zh-Hans",
        "zh-hant" | "zh-cht" | "zh-tw" | "zh-hk" | "zh-mo" | "cht" | "tw" => "zh-Hant",
        value if value == "vi" || value.starts_with("vi-") => "vi",
        value if value == "it" || value.starts_with("it-") => "it",
        value if value == "es" || value.starts_with("es-") => "es",
        value if value == "pt" || value.starts_with("pt-") => "pt",
        value if value == "hi" || value.starts_with("hi-") => "hi",
        value if value == "th" || value.starts_with("th-") => "th",
        value
            if value == "id"
                || value.starts_with("id-")
                || value == "in"
                || value.starts_with("in-") =>
        {
            "id"
        }
        _ => "en",
    }
}

pub(crate) fn memorial_notice_copy(locale: &str) -> (&'static str, &'static str) {
    match locale {
        "ko" => (
            "Guitar Girl 팬 메모리얼 빌드",
            "비공식·비상업용 보존판입니다. 게임 서비스는 기기 안에서만 실행됩니다.",
        ),
        "ja" => (
            "Guitar Girl ファンメモリアルビルド",
            "非公式・非営利の保存版です。ゲームサービスは端末内だけで動作します。",
        ),
        "zh-Hans" => (
            "吉他少女 Fan Memorial Build",
            "非官方、非商业的停运纪念版。所有游戏服务仅在本机运行。",
        ),
        "zh-Hant" => (
            "吉他少女 Fan Memorial Build",
            "非官方、非商業的停運紀念版。所有遊戲服務僅在本機執行。",
        ),
        "vi" => (
            "Bản tưởng niệm Guitar Girl",
            "Bản lưu niệm không chính thức, phi thương mại. Dịch vụ chỉ chạy trên thiết bị.",
        ),
        "it" => (
            "Guitar Girl Fan Memorial Build",
            "Edizione non ufficiale e non commerciale. Servizi solo sul dispositivo.",
        ),
        "es" => (
            "Guitar Girl Fan Memorial Build",
            "Edición no oficial y no comercial. Servicios solo en este dispositivo.",
        ),
        "pt" => (
            "Guitar Girl Fan Memorial Build",
            "Edição não oficial e não comercial. Serviços apenas neste dispositivo.",
        ),
        "hi" => (
            "Guitar Girl Fan Memorial Build",
            "अनौपचारिक, गैर-व्यावसायिक स्मारक संस्करण। सभी सेवाएँ केवल इस डिवाइस पर चलती हैं।",
        ),
        "th" => (
            "Guitar Girl Fan Memorial Build",
            "ฉบับอนุสรณ์ไม่เป็นทางการและไม่ใช่เชิงพาณิชย์ บริการทำงานบนอุปกรณ์นี้เท่านั้น",
        ),
        "id" => (
            "Guitar Girl Fan Memorial Build",
            "Edisi tidak resmi dan nonkomersial. Semua layanan hanya di perangkat ini.",
        ),
        _ => (
            "Guitar Girl Fan Memorial Build",
            "Unofficial, non-commercial preservation build. Game services run only on this device.",
        ),
    }
}

fn memorial_notice_features(locale: &str) -> &'static str {
    match locale {
        "ko" => "설정 → 메모리얼 기능: 단종 의상·기타와 재화를 우편으로 받기. 패스 시즌 변경은 재시작 후 적용됩니다.",
        "ja" => "設定 → メモリアル機能：限定衣装・ギターと資源をメールで受取。パスの期変更は再起動で反映。",
        "zh-Hans" => "设置 → 纪念版功能：邮件领取绝版服装、吉他及资源；切换通行证期数（重启生效）。",
        "zh-Hant" => "設定 → 紀念版功能：郵件領取絕版服裝、吉他及資源；切換通行證期數（重啟套用）。",
        "vi" => "Cài đặt → Tính năng tưởng niệm: nhận đồ hiếm và tài nguyên qua thư. Đổi mùa pass: khởi động lại.",
        "it" => "Impostazioni → Funzioni commemorative: abiti e chitarre ritirati, risorse via posta. Cambio pass: riavvia.",
        "es" => "Ajustes → Funciones conmemorativas: ropa y guitarras retiradas, recursos por correo. Cambiar pase: reinicia.",
        "pt" => "Configurações → Recursos comemorativos: roupas e guitarras antigas, recursos por correio. Trocar passe: reinicie.",
        "hi" => "सेटिंग्स → स्मृति सुविधाएँ: पुराने परिधान, गिटार और संसाधन मेल से लें। पास सीज़न बदलकर गेम फिर खोलें।",
        "th" => "การตั้งค่า → ฟีเจอร์อนุสรณ์: รับชุด กีตาร์เลิกแจก และทรัพยากรทางจดหมาย เปลี่ยนฤดูพาสแล้วเริ่มเกมใหม่",
        "id" => "Pengaturan → Fitur Memorial: kostum/gitar langka dan sumber daya lewat surat. Ganti musim pass: mulai ulang.",
        _ => "Settings → Memorial Features: retired outfits/guitars and resources by mail. Switch pass season, then restart.",
    }
}


fn memorial_maintenance_value(locale: &str) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    let (title, body) = memorial_notice_copy(normalized_notice_locale(locale));
    let body = format!("{body}\n\n{}", memorial_notice_features(normalized_notice_locale(locale)));
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "common_model",
        "MaintenanceData",
        [
            ("Code".into(), Value::I16(1)),
            ("Title".into(), Value::String(title.as_bytes().to_vec())),
            (
                "Description".into(),
                Value::String(body.as_bytes().to_vec()),
            ),
            ("Utc_time".into(), Value::I16(9)),
            ("Facebook_url".into(), Value::String(Vec::new())),
            (
                "Start_datetime".into(),
                Value::String(b"2026-09-04 00:00:00".to_vec()),
            ),
            (
                "End_datetime".into(),
                Value::String(b"2099-12-31 23:59:59".to_vec()),
            ),
        ],
    )
}

fn take_startup_notice(gate: &AtomicBool) -> bool {
    !gate.swap(true, Ordering::AcqRel)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"ok":true,"service":"ggfm","abi":1}))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    usn: i64,
    user_id: String,
    nonce: i64,
    protocol_version: u32,
    master_schema_version: i64,
    master_source_tables: i64,
}

async fn current_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(session) = state.session.lock().await.clone() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    Json(SessionView {
        usn: session.identity.usn,
        user_id: session.identity.user_id.clone(),
        nonce: session.nonce,
        protocol_version: ggfm_protocol::PROTOCOL_VERSION,
        master_schema_version: state.master.metadata.version,
        master_source_tables: state.master.metadata.source_table_count,
    })
    .into_response()
}

async fn list_slots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state
        .database
        .execute(|database| database.slot_views())
        .await
    {
        Ok(slots) => Json(slots).into_response(),
        Err(error) => internal_error(error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSlot {
    display_name: String,
}

async fn create_slot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSlot>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let name = request.display_name.trim();
    if name.is_empty() || name.chars().count() > 32 {
        return (StatusCode::BAD_REQUEST, "invalid displayName").into_response();
    }
    let (_, _, mutation) = match admin_write_context(&state, &headers, "memorial.createSlot").await
    {
        Ok(context) => context,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let name = name.to_owned();
    match state
        .database
        .execute(move |database| database.create_slot_rpc(&mutation, &name))
        .await
    {
        Ok(identity) => (StatusCode::CREATED, Json(identity)).into_response(),
        Err(error) => admin_mutation_error(error),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectSlotResponse {
    pending_usn: i64,
    restart_required: bool,
}

async fn select_slot(
    State(state): State<AppState>,
    Path(usn): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (_, _, mutation) = match admin_write_context(&state, &headers, "memorial.selectSlot").await
    {
        Ok(context) => context,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let selector = usn.to_string();
    match state
        .database
        .execute(move |database| database.request_switch_rpc(&mutation, &selector))
        .await
    {
        Ok(_) => Json(SelectSlotResponse {
            pending_usn: usn,
            restart_required: true,
        })
        .into_response(),
        Err(ggfm_persistence_sqlite::StoreError::MissingSlot(_)) => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => admin_mutation_error(error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSlotRequest {
    confirm_display_name: String,
}

async fn delete_slot(
    State(state): State<AppState>,
    Path(usn): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<DeleteSlotRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (_, _, mutation) = match admin_write_context(&state, &headers, "memorial.deleteSlot").await
    {
        Ok(context) => context,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let confirmation = request.confirm_display_name.trim().to_owned();
    match state
        .database
        .execute(move |database| database.delete_inactive_slot_rpc(&mutation, usn, &confirmation))
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(ggfm_persistence_sqlite::StoreError::MissingSlot(_)) => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(ggfm_persistence_sqlite::StoreError::ActiveSlot(_)) => (
            StatusCode::CONFLICT,
            "active or pending slot cannot be deleted",
        )
            .into_response(),
        Err(ggfm_persistence_sqlite::StoreError::Domain(
            ggfm_domain::DomainError::IdentityMismatch,
        )) => (StatusCode::BAD_REQUEST, "typed confirmation does not match").into_response(),
        Err(error) => admin_mutation_error(error),
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyItemView {
    kind: ContentKind,
    area: i32,
    id: i64,
    names: BTreeMap<String, String>,
    source: &'static str,
    owned: bool,
    pending_mail: bool,
    can_send: bool,
}

async fn list_legacy_items(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(session) = state.session.lock().await.clone() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let mut items = legacy_items(&state);
    match state.database.execute(move |database| {
        for item in &mut items {
            (item.owned, item.pending_mail) = database.content_delivery_state(
                session.identity.usn, item.kind, item.area, item.id,
            )?;
            item.can_send = !item.owned && !item.pending_mail;
        }
        Ok(items)
    }).await {
        Ok(items) => Json(items).into_response(),
        Err(error) => admin_mutation_error(error),
    }
}

async fn send_legacy_item(
    State(state): State<AppState>,
    Path((kind, area, id)): Path<(String, i32, i64)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(content) = parse_content_kind(&kind) else {
        return (StatusCode::BAD_REQUEST, "unknown content kind").into_response();
    };
    if !legacy_items(&state)
        .iter()
        .any(|item| item.kind == content && item.area == area && item.id == id)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (_, _, mutation) =
        match admin_write_context(&state, &headers, "memorial.sendLegacyItem").await {
            Ok(context) => context,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        };
    let reward = Reward::Content {
        content,
        area,
        content_id: id,
    };
    let result = state
        .database
        .execute(move |database| {
            database.queue_generated_mail_rpc(
                &mutation,
                "memorial.legacy.subject",
                "memorial.legacy.body",
                &reward,
            )
        })
        .await;
    mail_enqueue_response(result)
}

#[derive(Deserialize)]
struct CurrencyMailRequest {
    amount: String,
}

async fn send_currency_mail(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CurrencyMailRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(currency) = parse_currency_kind(&kind) else {
        return (StatusCode::BAD_REQUEST, "unknown currency kind").into_response();
    };
    let Ok(amount) = parse_currency_amount(currency, &request.amount) else {
        return (StatusCode::BAD_REQUEST, "invalid amount").into_response();
    };
    let (_, _, mutation) =
        match admin_write_context(&state, &headers, "memorial.sendCurrency").await {
            Ok(context) => context,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        };
    let reward = Reward::Currency { currency, amount };
    let result = state
        .database
        .execute(move |database| {
            database.queue_generated_mail_rpc(
                &mutation,
                "memorial.currency.subject",
                "memorial.currency.body",
                &reward,
            )
        })
        .await;
    mail_enqueue_response(result)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StarPassView {
    seasons: Vec<i64>,
    active_season: i64,
    anchor_day: i64,
    restart_required_after_selection: bool,
}

async fn list_star_passes(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(session) = state.session.lock().await.clone() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let clock = request_clock(&headers, session.clock, false).unwrap_or(session.clock);
    let usn = session.identity.usn;
    match state
        .database
        .execute(move |database| database.pass_snapshot(usn))
        .await
    {
        Ok(pass) => Json(StarPassView {
            seasons: state
                .master
                .pass_seasons
                .iter()
                .map(|row| row.season)
                .collect(),
            active_season: ggfm_policy_memorial::pass_season(
                clock,
                pass.anchor.anchor_day,
                pass.anchor.anchor_season,
            ),
            anchor_day: pass.anchor.anchor_day,
            restart_required_after_selection: true,
        })
        .into_response(),
        Err(error) => internal_error(error),
    }
}

async fn select_star_pass(
    State(state): State<AppState>,
    Path(season): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !state
        .master
        .pass_seasons
        .iter()
        .any(|row| row.season == season)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (_, clock, mutation) =
        match admin_write_context(&state, &headers, "memorial.selectPass").await {
            Ok(context) => context,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        };
    match state
        .database
        .execute(move |database| database.select_pass_season_rpc(&mutation, clock, season))
        .await
    {
        Ok(()) => Json(serde_json::json!({
            "selectedSeason": season,
            "anchorDay": clock.local_epoch_day(),
            "restartRequired": true
        }))
        .into_response(),
        Err(error) => admin_mutation_error(error),
    }
}

async fn admin_write_context(
    state: &AppState,
    headers: &HeaderMap,
    rpc: &str,
) -> Result<(LoginSession, DeviceClock, RpcMutation), &'static str> {
    let session = state
        .session
        .lock()
        .await
        .clone()
        .ok_or("no active login session")?;
    let clock = request_clock(headers, session.clock, true)?;
    let request_seq = header_i64(headers, REQUEST_SEQ_HEADER)
        .filter(|value| *value > 0)
        .ok_or("missing or invalid request sequence")?;
    let mutation = RpcMutation {
        nonce: session.nonce,
        identity: session.identity.clone(),
        request_seq,
        rpc: rpc.to_owned(),
        idempotency_key: format!("admin:{rpc}:{request_seq}"),
        committed_at: clock.unix_seconds,
    };
    Ok((session, clock, mutation))
}

fn mail_enqueue_response(result: Result<i64, ggfm_persistence_sqlite::StoreError>) -> Response {
    match result {
        Ok(post_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"postId":post_id})),
        )
            .into_response(),
        Err(ggfm_persistence_sqlite::StoreError::Domain(
            ggfm_domain::DomainError::AlreadyOwned,
        )) => (StatusCode::CONFLICT, "already owned").into_response(),
        Err(error) => admin_mutation_error(error),
    }
}

fn admin_mutation_error(error: StoreError) -> Response {
    match error {
        StoreError::Domain(ggfm_domain::DomainError::StaleRequest) => {
            (StatusCode::CONFLICT, "stale request sequence").into_response()
        }
        StoreError::IdempotencyConflict => {
            (StatusCode::CONFLICT, "idempotency conflict").into_response()
        }
        error => internal_error(error),
    }
}

fn legacy_items(state: &AppState) -> Vec<LegacyItemView> {
    legacy_catalog(&state.master)
}

fn legacy_catalog(master: &MasterCatalog) -> Vec<LegacyItemView> {
    let protected = protected_reward_content(master);
    let mut result = Vec::new();
    // AcquisitionType=3 means Star Pass, not discontinued content. Do not
    // infer eligibility from it. The reviewed policy names the tested set;
    // the user's master still controls presence, area, and active status.
    for item in &ggfm_policy_memorial::MemorialPolicy::embedded().legacy_items {
        if protected.contains(&(item.kind, item.id)) { continue; }
        let (table, exists) = match item.kind {
            ContentKind::Costume => ("Costume", master.costumes.iter().any(|row|
                row.active && row.id == item.id && row.area == item.area)),
            ContentKind::Guitar => ("Guitar", master.guitars.iter().any(|row|
                row.active && row.id == item.id && row.area == item.area)),
            ContentKind::Music => ("Music", master.music.iter().any(|row|
                row.active && row.id == item.id && row.area == item.area)),
            _ => continue,
        };
        if exists { result.push(legacy_item(master, item.kind, table, item.area, item.id)); }
    }
    result
}

fn protected_reward_content(master: &MasterCatalog) -> BTreeSet<(ContentKind, i64)> {
    let mut reward_groups = BTreeSet::new();
    for row in &master.pass_rewards {
        reward_groups.insert(row.free_reward_group);
        reward_groups.insert(row.paid_reward_group);
    }
    for row in &master.ch3_stages {
        reward_groups.insert(row.reward_group_id);
    }
    for row in &master.ch3_chapters {
        reward_groups.extend(row.reward_groups);
    }
    let mut result = BTreeSet::new();
    for row in &master.reward_groups {
        if reward_groups.contains(&row.group) {
            insert_content_reward(&mut result, row.reward_type, row.reward_id);
        }
    }
    // Retired login/percentage-event rewards are deliberately in the reviewed
    // legacy list. Only live CH3 and rotating-pass sources are protected here.
    result
}

fn insert_content_reward(
    result: &mut BTreeSet<(ContentKind, i64)>,
    reward_type: i16,
    reward_id: i32,
) {
    let kind = match reward_type {
        2 => ContentKind::Music,
        3 => ContentKind::Costume,
        9 => ContentKind::Guitar,
        _ => return,
    };
    result.insert((kind, i64::from(reward_id)));
}

fn legacy_item(
    master: &MasterCatalog,
    kind: ContentKind,
    table: &str,
    area: i32,
    id: i64,
) -> LegacyItemView {
    let names = master
        .game_data
        .get(table)
        .and_then(|rows| {
            rows.iter()
                .find(|row| master_row_i64(row, "id") == Some(id))
        })
        .map(localized_names)
        .unwrap_or_default();
    LegacyItemView {
        kind,
        area,
        id,
        names,
        source: "legacy-no-longer-distributed",
        owned: false,
        pending_mail: false,
        can_send: false,
    }
}

fn localized_names(row: &MasterRow) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for (locale, suffix) in [
        ("en", "en"), ("ko", "ko"), ("ja", "ja"), ("zh-Hans", "zhchs"),
        ("zh-Hant", "zhcht"), ("vi", "vi"), ("es", "es"), ("it", "it"),
        ("id", "id"), ("th", "th"), ("pt", "pt"), ("hi", "hi"),
    ] {
        for prefix in ["name", "title"] {
            if let Some(MasterScalar::Text(text)) = row.get(&format!("{prefix}{suffix}"))
                && !text.is_empty()
            {
                result.insert(locale.to_owned(), text.clone());
                break;
            }
        }
    }
    result
}

fn master_row_i64(row: &MasterRow, field: &str) -> Option<i64> {
    match row.get(field)? {
        MasterScalar::Integer(value) => Some(*value),
        MasterScalar::Real(value) => Some(*value as i64),
        MasterScalar::Text(value) => value.parse().ok(),
        MasterScalar::Null | MasterScalar::Blob(_) => None,
    }
}

fn parse_content_kind(value: &str) -> Option<ContentKind> {
    Some(match value.to_ascii_lowercase().as_str() {
        "costume" => ContentKind::Costume,
        "guitar" => ContentKind::Guitar,
        "music" => ContentKind::Music,
        _ => return None,
    })
}

fn parse_currency_kind(value: &str) -> Option<CurrencyKind> {
    CurrencyKind::ALL
        .into_iter()
        .find(|candidate| candidate.key() == value.to_ascii_lowercase())
}

fn parse_currency_amount(currency: CurrencyKind, value: &str) -> Result<f64, ()> {
    // Memorial input is an integer multiplier or a literal integer count.
    // Do not reuse the game's A/B/K display notation or expression parser.
    if value.is_empty() || value.len() > 64 {
        return Err(());
    }
    let literal = matches!(currency, CurrencyKind::Candy | CurrencyKind::Chocolate);
    if !value.bytes().all(|c| c.is_ascii_digit()) {
        return Err(());
    }
    let result: f64 = value.parse().map_err(|_| ())?;
    if result.is_finite() && result > 0.0 && (!literal || result <= f64::from(i32::MAX)) {
        Ok(result)
    } else {
        Err(())
    }
}

async fn legacy_rpc(
    State(state): State<AppState>,
    Path((category, route_rpc, locale)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    runtime_log(&format!(
        "request: POST /{category}/{route_rpc}/{locale}/ bytes={} session_header={}",
        body.len(),
        headers.contains_key(SESSION_HEADER)
    ));
    if !authorized(&state, &headers) {
        runtime_log("request rejected: invalid session capability");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let form = parse_game_form(&body);
    let Some(wire_rpc) = resolve_wire_rpc(&route_rpc, &form) else {
        runtime_log("request rejected: generic Request route has no call field");
        return (StatusCode::BAD_REQUEST, "missing call").into_response();
    };
    // The launch bootstrap is the one historical exception to the generated
    // RPC naming: its HTTP/form call is literally `Request`, while the typed
    // client contract carried by that envelope is `main.init`.
    let rpc = if category == "main" && wire_rpc.eq_ignore_ascii_case("Request") {
        "init"
    } else {
        wire_rpc
    };
    runtime_log(&format!("request dispatch: category={category} call={rpc}"));
    let Some(rpc_name) = RpcName::from_wire_name(rpc) else {
        runtime_log(&format!("request rejected: unknown RPC {rpc}"));
        return StatusCode::NOT_FOUND.into_response();
    };
    let spec = ProtocolSchema::embedded().call_typed(rpc_name);
    if rpc_name.category() != category {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(handler) = handler_for(rpc_name) else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    let Some(session) = state.session.lock().await.clone() else {
        runtime_log("request rejected: no active login session");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if rpc_name.class() == ggfm_protocol::RpcClass::Active && !gameplay::is_implemented(rpc_name) {
        tracing::error!(%category, %rpc, "active RPC has no gameplay implementation");
        return thrift_error(
            spec,
            session.clock.unix_seconds,
            992,
            "active RPC is not implemented by this development build",
        );
    }
    if !route_rpc.eq_ignore_ascii_case("request")
        && form.call.as_deref().is_some_and(|call| call != wire_rpc)
    {
        return thrift_error(spec, session.clock.unix_seconds, 997, "form call mismatch");
    }
    let raw_request = match form
        .tapsonic_data
        .as_deref()
        .ok_or_else(|| "missing tapsonic_data".to_owned())
        .and_then(|encoded| decode_transport(encoded).map_err(|error| error.to_string()))
    {
        Ok(raw) => raw,
        Err(error) => {
            runtime_log(&format!(
                "request rejected: transport decode failed: {error}"
            ));
            tracing::warn!(%category, %rpc, %error, "rejected malformed game RPC");
            return thrift_error(spec, session.clock.unix_seconds, 997, &error);
        }
    };
    let request = match decode_request(&raw_request, spec, ProtocolSchema::embedded()) {
        Ok(request) => request,
        Err(error) => {
            runtime_log(&format!("request rejected: Thrift decode failed: {error}"));
            tracing::warn!(%category, %rpc, %error, "rejected malformed game RPC");
            return thrift_error(spec, session.clock.unix_seconds, 997, &error.to_string());
        }
    };
    if let Err(message) = validate_request_identity(
        &request,
        &session.identity,
        matches!(rpc, "userLogin" | "userJoin"),
    ) {
        runtime_log(&format!("request rejected: identity: {message}"));
        return thrift_error(spec, session.clock.unix_seconds, 996, message);
    }
    let clock = match request_clock(&headers, session.clock, handler.mutating) {
        Ok(clock) => clock,
        Err(message) => return thrift_error(spec, session.clock.unix_seconds, 995, message),
    };
    let sequence = header_i64(&headers, REQUEST_SEQ_HEADER).filter(|value| *value > 0);
    if handler.mutating && sequence.is_none() {
        return thrift_error(spec, clock.unix_seconds, 994, "missing request sequence");
    }
    let data = match gameplay::execute(
        &state,
        &session,
        gameplay::GameplayCall {
            rpc: rpc_name,
            locale: &locale,
            request: &request,
            clock,
            sequence,
            raw_request: &raw_request,
        },
    )
    .await
    {
        Ok(data) => data,
        Err(error) => {
            runtime_log(&format!("request rejected: gameplay handler: {error}"));
            tracing::warn!(%category, %rpc, %error, "gameplay RPC rejected");
            return thrift_error(spec, clock.unix_seconds, 993, &error.to_string());
        }
    };
    if data.is_none() && spec.rpc_class() == ggfm_protocol::RpcClass::Active {
        tracing::error!(%category, %rpc, "active RPC has no gameplay implementation");
        return thrift_error(
            spec,
            clock.unix_seconds,
            992,
            "active RPC is not implemented by this development build",
        );
    }

    let response = if rpc_name == RpcName::GetUpdateTime
        && take_startup_notice(&state.startup_notice_emitted)
    {
        runtime_log("startup notice: emitting once for this process");
        memorial_maintenance_value(&locale).and_then(|maintenance| {
            success_response_with_maintenance(
                spec,
                ProtocolSchema::embedded(),
                clock.unix_seconds,
                data,
                maintenance,
            )
        })
    } else {
        if rpc_name == RpcName::GetUpdateTime {
            runtime_log("startup notice: duplicate update-time request suppressed");
        }
        success_response(spec, ProtocolSchema::embedded(), clock.unix_seconds, data)
    };
    match response.and_then(|raw| encode_transport(&raw).map_err(Into::into)) {
        Ok(encoded) => {
            runtime_log(&format!(
                "request complete: call={rpc} response_bytes={}",
                encoded.len()
            ));
            text_payload(encoded)
        }
        Err(error) => {
            runtime_log(&format!("request failed: response encode: {error}"));
            tracing::error!(%category, %rpc, %error, "failed to encode game response");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Default)]
struct GameForm {
    call: Option<String>,
    tapsonic_data: Option<String>,
}

fn resolve_wire_rpc<'a>(route_rpc: &'a str, form: &'a GameForm) -> Option<&'a str> {
    if route_rpc.eq_ignore_ascii_case("request") {
        form.call.as_deref()
    } else {
        Some(route_rpc)
    }
}

fn parse_game_form(body: &[u8]) -> GameForm {
    let mut result = GameForm::default();
    for (key, value) in form_urlencoded::parse(body) {
        match key.as_ref() {
            "call" => result.call = Some(value.into_owned()),
            "tapsonic_data" => result.tapsonic_data = Some(value.into_owned()),
            _ => {}
        }
    }
    result
}

fn validate_request_identity<'a>(
    request: &ggfm_protocol::NamedRequest,
    identity: &UserIdentity,
    allow_identity_discovery: bool,
) -> Result<(), &'a str> {
    if let Some(usn) = request.integer("U_seq")
        && usn != identity.usn
        && !allow_identity_discovery
    {
        return Err("U_seq does not match the active save slot");
    }
    if let Some(user_id) = request.string("U_id")
        && !user_id.is_empty()
        && user_id != identity.user_id
        && !allow_identity_discovery
    {
        return Err("U_id does not match the active save slot");
    }
    Ok(())
}

fn request_clock(
    headers: &HeaderMap,
    fallback: DeviceClock,
    required: bool,
) -> Result<DeviceClock, &'static str> {
    match (
        header_i64(headers, DEVICE_TIME_HEADER),
        header_i64(headers, UTC_OFFSET_HEADER).and_then(|value| i32::try_from(value).ok()),
    ) {
        (Some(unix), Some(offset)) => {
            DeviceClock::new(unix, offset).map_err(|_| "invalid device clock")
        }
        (None, None) if !required => Ok(fallback),
        _ => Err("missing device time headers"),
    }
}

fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn thrift_error(spec: &ggfm_protocol::RpcSpec, now: i64, code: i32, message: &str) -> Response {
    match error_response(spec, now, code, message)
        .and_then(|raw| encode_transport(&raw).map_err(Into::into))
    {
        Ok(encoded) => text_payload(encoded),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn text_payload(encoded: String) -> Response {
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        encoded,
    )
        .into_response()
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_equal(value.as_bytes(), state.capability.as_bytes()))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn internal_error(error: impl std::fmt::Display) -> Response {
    tracing::error!(%error, "memorial server database error");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_notice_explains_memorial_settings_in_all_languages() {
        for locale in ["en","ko","ja","zh-Hans","zh-Hant","vi","it","es","pt","hi","th","id"] {
            let hint = memorial_notice_features(locale);
            assert!(hint.len() > 80);
            if locale != "en" { assert_ne!(hint, memorial_notice_features("en")); }
            assert!(format!("{:?}", memorial_maintenance_value(locale).unwrap()).len() > hint.len());
        }
        let hint = memorial_notice_features("zh-Hans");
        for term in ["设置", "纪念版功能", "邮件", "服装", "吉他", "资源", "通行证", "重启"] {
            assert!(hint.contains(term));
        }
    }
    #[test]
    fn memorial_currency_input_is_integer_multiplier_or_literal_count() {
        for currency in CurrencyKind::ALL {
            for value in ["1", "10", "1000"] {
                assert!(parse_currency_amount(currency, value).is_ok());
            }
            for value in ["1A", "1k", "1K", "1+2", "1e3", "-1", "+1", "0", ".5", "1.", " 1", "NaN", "1..2"] {
                assert!(parse_currency_amount(currency, value).is_err(), "{currency:?}: {value}");
            }
            let literal = matches!(currency, CurrencyKind::Candy | CurrencyKind::Chocolate);
            assert!(parse_currency_amount(currency, "2.5").is_err());
            if literal {
                assert!(parse_currency_amount(currency, "2147483648").is_err());
            }
        }
    }
    #[test]
    fn memorial_legacy_catalog_uses_reviewed_ids_and_client_locale_keys() {
        use ggfm_master_data::CostumeRow;
        let mut master = MasterCatalog::default();
        let costume = |id, area, active, acquisition_type| CostumeRow {
            id, area, active, acquisition_type, acquisition_id: 0,
            currency: "NotForSale".into(), official_cost: "0".into(), fan_multiplier: 0.3,
        };
        master.costumes = vec![
            costume(12, 1, true, 1), // Sunflower: event, not Star Pass.
            costume(11, 1, true, 3), // Star Pass never appears here.
            costume(20, 1, false, 1), // Inactive resources are not advertised.
            costume(203, 1, true, 1), // Wrong chapter fails closed.
        ];
        master.game_data.insert("Costume".into(), vec![BTreeMap::from([
            ("id".into(), MasterScalar::Integer(12)),
            ("nameen".into(), MasterScalar::Text("Fixture sunflower".into())),
            ("namezhchs".into(), MasterScalar::Text("测试向日葵".into())),
        ])]);
        let items = legacy_catalog(&master);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].kind, items[0].area, items[0].id), (ContentKind::Costume, 1, 12));
        assert_eq!(items[0].names.get("en").map(String::as_str), Some("Fixture sunflower"));
        assert_eq!(items[0].names.get("zh-Hans").map(String::as_str), Some("测试向日葵"));
        assert!(!items[0].names.contains_key("namezhchs"));
        let wire = serde_json::to_value(items).unwrap();
        assert_eq!(wire[0]["kind"], "costume");
        assert_eq!(wire[0]["names"]["zh-Hans"], "测试向日葵");
    }

    use super::*;
    use ggfm_persistence_sqlite::{Database, DatabaseActor};
    use ggfm_protocol::{
        Field, RpcClass, Value, decode_transport, encode_transport, read_struct, write_struct,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn startup_failure_reports_the_real_stage_without_false_ready_logs() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let missing = std::env::temp_dir().join(format!("ggfm-missing-master-{suffix}.sqlite"));
        let database = std::env::temp_dir().join(format!("ggfm-not-opened-{suffix}.sqlite"));
        let messages = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = messages.clone();
        let result = start(ServerConfig {
            database_path: database.clone(), master_data_path: missing,
            capability: "a".repeat(64), clock: DeviceClock::new(100, 0).unwrap(),
            asset_source: None,
            progress: Some(Arc::new(move |line| sink.lock().unwrap().push(line.to_owned()))),
        }).await;
        assert!(result.is_err());
        assert!(!database.exists());
        let messages = messages.lock().unwrap();
        assert!(messages.first().unwrap().starts_with("[INFO] master:"));
        assert!(messages.last().unwrap().starts_with("[ERROR] master:"));
        assert!(!messages.iter().any(|line| line.contains("listening") || line.contains("integrity check passed")));
    }

    #[test]
    fn connection_diagnostics_report_the_actual_sqlite_configuration() {
        let database = Database::open_memory().unwrap();
        assert_eq!(database.connection_diagnostics().unwrap(),
            "schema=4 journal=memory foreign_keys=1 synchronous=2");
    }

    #[tokio::test]
    async fn corrupt_primary_recovers_from_consistent_backup() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("ggfm-recovery-{suffix}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("ggfm.sqlite3");
        let backup = path.with_extension("sqlite3.backup");
        let identity = {
            let mut database = Database::open(&path).unwrap();
            let identity = database.create_slot("Recovery", 100).unwrap();
            database
                .grant_currency(
                    identity.usn,
                    CurrencyKind::Chocolate,
                    23.0,
                    "recovery-seed",
                    101,
                )
                .unwrap();
            database.backup_to(&backup).unwrap();
            identity
        };
        fs::write(&path, b"not a sqlite database").unwrap();
        let recovered = open_database_with_recovery(&path, 200).unwrap();
        let usn = identity.usn;
        assert_eq!(
            recovered
                .execute(move |database| database.currency(usn, CurrencyKind::Chocolate))
                .await
                .unwrap(),
            23.0
        );
        drop(recovered);
        for entry in fs::read_dir(&directory).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn all_127_audited_rpcs_route_and_return_valid_thrift_over_http_boundary() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-http-coverage-{suffix}.sqlite"));
        let database = DatabaseActor::open(path.clone()).unwrap();
        database
            .execute(|database| database.ensure_default_slot(1_788_360_000))
            .await
            .unwrap();
        let session = database
            .execute(|database| {
                database.begin_login(
                    b"http-coverage",
                    DeviceClock::new(1_788_360_000, 0).unwrap(),
                )
            })
            .await
            .unwrap();
        let state = AppState {
            database: database.clone(),
            master: Arc::new(MasterCatalog::default()),
            session: Arc::new(Mutex::new(Some(session))),
            capability: Arc::from("http-coverage"),
            capability_hash: Arc::from(Vec::<u8>::new()),
            endpoint: Arc::from("http://127.0.0.1:1"),
            asset_source: None,
            startup_notice_emitted: Arc::new(AtomicBool::new(false)),
        };

        for (index, rpc) in RpcName::ALL.into_iter().enumerate() {
            let raw = write_struct(&[
                Field {
                    id: 1,
                    value: Value::String(rpc.as_str().as_bytes().to_vec()),
                },
                Field {
                    id: 2,
                    value: Value::Struct(Vec::new()),
                },
            ])
            .unwrap();
            let encoded = encode_transport(&raw).unwrap();
            let form_call = if rpc == RpcName::Init {
                "Request"
            } else {
                rpc.as_str()
            };
            let body = form_urlencoded::Serializer::new(String::new())
                .append_pair("call", form_call)
                .append_pair("tapsonic_data", &encoded)
                .finish();
            let mut headers = HeaderMap::new();
            headers.insert(SESSION_HEADER, "http-coverage".parse().unwrap());
            headers.insert(REQUEST_SEQ_HEADER, (index + 1).to_string().parse().unwrap());
            headers.insert(DEVICE_TIME_HEADER, "1788360100".parse().unwrap());
            headers.insert(UTC_OFFSET_HEADER, "0".parse().unwrap());

            let response = legacy_rpc(
                State(state.clone()),
                Path((
                    rpc.category().to_owned(),
                    form_call.to_owned(),
                    "en".to_owned(),
                )),
                headers,
                Bytes::from(body),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{} must complete at the original HTTP boundary",
                rpc.as_str()
            );
            let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            let encoded = std::str::from_utf8(&body).unwrap();
            let decoded = decode_transport(encoded)
                .unwrap_or_else(|error| panic!("{} transport: {error}", rpc.as_str()));
            let envelope = read_struct(&decoded)
                .unwrap_or_else(|error| panic!("{} envelope: {error}", rpc.as_str()));
            assert!(
                envelope.iter().any(|field| field.id == 1),
                "{} must return the original result wrapper",
                rpc.as_str()
            );
            if rpc.class() != RpcClass::Active {
                let result = envelope
                    .iter()
                    .find(|field| field.id == 1)
                    .and_then(|field| match &field.value {
                        Value::Struct(fields) => fields.first(),
                        _ => None,
                    })
                    .map(|field| &field.value);
                assert_eq!(
                    result,
                    Some(&Value::I32(0)),
                    "{} compatibility handler must remain a successful no-op",
                    rpc.as_str()
                );
            }
        }

        drop(state);
        drop(database);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn token_compare_is_length_and_content_sensitive() {
        assert!(constant_time_equal(b"abc", b"abc"));
        assert!(!constant_time_equal(b"abc", b"abd"));
        assert!(!constant_time_equal(b"abc", b"ab"));
    }

    #[test]
    fn original_form_shape_is_parsed_without_losing_base64_plus() {
        let raw = write_struct(&[
            Field {
                id: 1,
                value: Value::String(b"main".to_vec()),
            },
            Field {
                id: 2,
                value: Value::Struct(Vec::new()),
            },
        ])
        .unwrap();
        let encoded = encode_transport(&raw).unwrap();
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("call", "main")
            .append_pair("tapsonic_data", &encoded)
            .finish();
        let form = parse_game_form(body.as_bytes());
        assert_eq!(form.call.as_deref(), Some("main"));
        assert_eq!(form.tapsonic_data.as_deref(), Some(encoded.as_str()));
    }

    #[test]
    fn original_generic_request_route_uses_form_call() {
        let form = GameForm {
            call: Some("main".to_owned()),
            tapsonic_data: Some("payload".to_owned()),
        };
        assert_eq!(resolve_wire_rpc("Request", &form), Some("main"));
        assert_eq!(resolve_wire_rpc("main", &form), Some("main"));
        assert_eq!(resolve_wire_rpc("request", &GameForm::default()), None);
    }

    #[test]
    fn bootstrap_request_wire_name_maps_to_typed_init_contract() {
        let category = "main";
        let wire_rpc = "Request";
        let rpc = if category == "main" && wire_rpc.eq_ignore_ascii_case("Request") {
            "init"
        } else {
            wire_rpc
        };
        assert_eq!(rpc, "init");
        assert_eq!(RpcName::from_wire_name(rpc), Some(RpcName::Init));
    }

    #[test]
    fn login_discovery_accepts_previous_slot_identity_but_commands_do_not() {
        let identity = UserIdentity {
            usn: 2,
            user_id: "1788360000002".to_owned(),
        };
        let request = ggfm_protocol::NamedRequest {
            call: "userLogin".to_owned(),
            fields: BTreeMap::from([
                ("U_seq".to_owned(), ggfm_protocol::Value::I32(1)),
                (
                    "U_id".to_owned(),
                    ggfm_protocol::Value::String(b"1788360000001".to_vec()),
                ),
            ]),
            raw_fields: Vec::new(),
        };
        assert!(validate_request_identity(&request, &identity, true).is_ok());
        assert_eq!(
            validate_request_identity(&request, &identity, false),
            Err("U_seq does not match the active save slot")
        );
    }

    #[test]
    fn notice_locale_accepts_client_and_android_variants() {
        assert_eq!(normalized_notice_locale("zh_CN"), "zh-Hans");
        assert_eq!(normalized_notice_locale("zh_chs"), "zh-Hans");
        assert_eq!(normalized_notice_locale("zh-TW"), "zh-Hant");
        assert_eq!(normalized_notice_locale("zh_cht"), "zh-Hant");
        assert_eq!(normalized_notice_locale("ko-KR"), "ko");
        assert_eq!(normalized_notice_locale("pt-BR"), "pt");
        assert_eq!(normalized_notice_locale("in-ID"), "id");
        assert_eq!(normalized_notice_locale("unknown"), "en");
    }

    #[test]
    fn startup_notice_is_once_per_server_process() {
        let first_process = AtomicBool::new(false);
        assert!(take_startup_notice(&first_process));
        assert!(!take_startup_notice(&first_process));

        let next_cold_process = AtomicBool::new(false);
        assert!(take_startup_notice(&next_cold_process));
    }

    #[test]
    fn every_supported_notice_locale_has_localized_copy() {
        for locale in [
            "en", "ko", "ja", "zh-Hans", "zh-Hant", "vi", "it", "es", "pt", "hi", "th", "id",
        ] {
            let (title, body) = memorial_notice_copy(locale);
            assert!(!title.trim().is_empty());
            assert!(!body.trim().is_empty());
        }
    }

    #[test]
    fn packaged_asset_paths_collapse_the_stock_duplicate_prefix() {
        assert_eq!(
            normalize_asset_name("/AssetBundles//AssetBundles/Android/Android.manifest").as_deref(),
            Some("AssetBundles/Android/Android.manifest")
        );
        assert_eq!(
            normalize_asset_name("/AssetBundles/Android/music/example.ab").as_deref(),
            Some("AssetBundles/Android/music/example.ab")
        );
    }

    #[test]
    fn packaged_asset_paths_cannot_escape_the_apk_namespace() {
        for path in [
            "/healthz",
            "/AssetBundles/../master.sqlite",
            "/AssetBundles/%2e%2e/master.sqlite",
            "/AssetBundles/C:/secret",
        ] {
            assert_eq!(normalize_asset_name(path), None, "{path}");
        }
    }

    #[test]
    fn packaged_assets_support_get_and_head_only() {
        assert_eq!(packaged_asset_is_head(&Method::GET), Some(false));
        assert_eq!(packaged_asset_is_head(&Method::HEAD), Some(true));
        assert_eq!(packaged_asset_is_head(&Method::POST), None);
    }

    #[test]
    fn memorial_shop_overlay_is_complete_and_uses_direct_server_purchase() {
        let mut master = MasterCatalog::default();
        master.game_data.insert("Shop".into(), Vec::new());
        master.game_data.insert("Reward_group".into(), Vec::new());

        apply_memorial_shop_policy(&mut master).expect("valid memorial shop overlay");

        assert_eq!(master.shops.len(), 8);
        assert_eq!(master.reward_groups.len(), 11);
        assert!(master.shops.iter().all(|row| {
            (50_001..=50_008).contains(&row.id)
                && row.product_type == 4
                && row.sell_type == 0
                && row.sell_value == 0
                && row.active
        }));
        let shop_rows = &master.game_data["Shop"];
        assert_eq!(shop_rows.len(), 8);
        assert!(shop_rows.iter().all(|row| {
            master_row_i64(row, "producttype") == Some(4)
                && master_row_i64(row, "selltype") == Some(0)
                && master_row_i64(row, "sellvalue") == Some(0)
                && master_row_i64(row, "isactive") == Some(1)
        }));
        assert_eq!(master.game_data["Reward_group"].len(), 11);

        apply_memorial_shop_policy(&mut master)
            .expect("an identical Patcher-produced overlay is accepted");
        assert_eq!(master.shops.len(), 8);
        assert_eq!(master.reward_groups.len(), 11);
    }
}
