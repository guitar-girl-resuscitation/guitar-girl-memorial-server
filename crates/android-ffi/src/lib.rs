use std::{
    collections::VecDeque,
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(target_os = "android")]
use std::{fs::OpenOptions, io::Write};

use ggfm_domain::DeviceClock;
#[cfg(target_os = "android")]
use ggfm_transport_http::AssetSource;

pub const ABI_VERSION: u32 = 1;
mod terminal_banner;

/// Offline import/export, excluded by the same mutex as server start.
/// # Safety
/// Both paths must be valid NUL-terminated UTF-8 for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ggfm_server_transfer_save(
    data_dir: *const c_char, staging_path: *const c_char, importing: bool,
) -> i32 {
    if data_dir.is_null() || staging_path.is_null() { return -1; }
    let guard = state().lock().unwrap();
    if guard.is_some() { return -2; }
    let dir = unsafe { CStr::from_ptr(data_dir) }.to_string_lossy();
    let staging = unsafe { CStr::from_ptr(staging_path) }.to_string_lossy();
    let db = PathBuf::from(dir.as_ref()).join("ggfm.sqlite3");
    let result = if importing {
        ggfm_persistence_sqlite::transfer::import(std::path::Path::new(staging.as_ref()), &db)
    } else {
        ggfm_persistence_sqlite::transfer::export(&db, std::path::Path::new(staging.as_ref()))
    };
    match result {
        Ok(()) => { boot_log(if importing { "save.import.committed" } else { "save.export.snapshot.ready" }); 0 }
        Err(error) => { boot_log(format!("[ERROR] save.transfer.failed: {error}")); -3 }
    }
}

/// Render into caller storage, with a trailing NUL. Returns required capacity
/// including NUL; an undersized buffer is not written. Valid before start.
///
/// # Safety
/// A non-null buffer must be writable for `capacity` bytes and not aliased.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ggfm_server_startup_banner(
    columns: u32, buffer: *mut c_char, capacity: usize,
) -> usize {
    let text = terminal_banner::render(columns);
    let required = text.len() + 1;
    if !buffer.is_null() && capacity >= required {
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast::<u8>(), text.len());
            *buffer.add(text.len()) = 0;
        }
    }
    required
}

enum Control {
    BeginLogin {
        clock: DeviceClock,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    PrepareShutdown {
        now: i64,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Stop,
}

struct RuntimeState {
    endpoint: CString,
    control: tokio::sync::mpsc::UnboundedSender<Control>,
    thread: Option<JoinHandle<()>>,
    active_usn: Arc<AtomicI64>,
    // The pointer remains owned by Android; retaining it documents the native
    // lifetime contract even though master.sqlite has already been streamed.
    _asset_manager: usize,
}

static STATE: OnceLock<Mutex<Option<RuntimeState>>> = OnceLock::new();
static STARTUP_LOGS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn startup_logs() -> &'static Mutex<VecDeque<String>> {
    STARTUP_LOGS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn boot_log(message: impl Into<String>) {
    let message = message.into();
    let message = if message.starts_with('[') {
        message
    } else if message.starts_with("fatal.") {
        format!("[ERROR] {message}")
    } else {
        format!("[INFO] {message}")
    };
    let mut logs = startup_logs().lock().unwrap();
    if logs.len() == 256 {
        logs.pop_front();
    }
    logs.push_back(message);
}

#[cfg(target_os = "android")]
struct AndroidAssetSource {
    manager: usize,
}

#[cfg(target_os = "android")]
unsafe impl Send for AndroidAssetSource {}
#[cfg(target_os = "android")]
unsafe impl Sync for AndroidAssetSource {}

#[cfg(target_os = "android")]
impl AssetSource for AndroidAssetSource {
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let name = CString::new(name).map_err(|_| "asset name contains NUL".to_owned())?;
        let asset = unsafe { AAssetManager_open(self.manager as *mut c_void, name.as_ptr(), 2) };
        if asset.is_null() {
            return Ok(None);
        }
        struct Asset(*mut c_void);
        impl Drop for Asset {
            fn drop(&mut self) {
                unsafe { AAsset_close(self.0) };
            }
        }
        let asset = Asset(asset);
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read =
                unsafe { AAsset_read(asset.0, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
            if read < 0 {
                return Err("failed reading packaged AssetBundle".into());
            }
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
        }
        Ok(Some(bytes))
    }

    fn len(&self, name: &str) -> Result<Option<u64>, String> {
        let name = CString::new(name).map_err(|_| "asset name contains NUL".to_owned())?;
        let asset = unsafe { AAssetManager_open(self.manager as *mut c_void, name.as_ptr(), 2) };
        if asset.is_null() {
            return Ok(None);
        }
        struct Asset(*mut c_void);
        impl Drop for Asset {
            fn drop(&mut self) {
                unsafe { AAsset_close(self.0) };
            }
        }
        let asset = Asset(asset);
        let length = unsafe { AAsset_getLength64(asset.0) };
        if length < 0 {
            return Err("failed reading packaged AssetBundle length".into());
        }
        Ok(Some(length as u64))
    }
}

fn state() -> &'static Mutex<Option<RuntimeState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_abi_version() -> u32 {
    ABI_VERSION
}

/// Uppercase SHA-256 of the exact memorial policy embedded in this server.
/// Patcher validates the same fingerprint before injecting the library.
#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_policy_sha256() -> *const c_char {
    static POLICY: OnceLock<CString> = OnceLock::new();
    POLICY
        .get_or_init(|| CString::new(ggfm_policy_memorial::POLICY_SHA256).unwrap())
        .as_ptr()
}

/// Starts the embedded loopback server for the current Android process.
///
/// # Safety
///
/// `data_dir_utf8` and `capability_utf8` must be valid, NUL-terminated UTF-8
/// strings for the duration of this call. `asset_manager` must be either null
/// or a borrowed `AAssetManager` owned by the calling Android application and
/// must remain valid until `ggfm_server_stop` returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ggfm_server_start(
    data_dir_utf8: *const c_char,
    asset_manager: *mut c_void,
    device_unix_seconds: i64,
    utc_offset_minutes: i32,
    capability_utf8: *const c_char,
) -> i32 {
    startup_logs().lock().unwrap().clear();
    boot_log("runtime.abi.ready");
    if data_dir_utf8.is_null() || capability_utf8.is_null() {
        boot_log("fatal.startup_argument");
        return -1;
    }
    let Ok(data_dir_text) = unsafe { CStr::from_ptr(data_dir_utf8) }.to_str() else {
        return -2;
    };
    let Ok(capability) = unsafe { CStr::from_ptr(capability_utf8) }.to_str() else {
        return -2;
    };
    if capability.len() != 64 || !capability.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return -2;
    }
    let Ok(clock) = DeviceClock::new(device_unix_seconds, utc_offset_minutes) else {
        return -3;
    };
    let mut guard = state().lock().unwrap();
    if guard.is_some() {
        boot_log("runtime.already_running");
        return 1;
    }
    let data_dir = PathBuf::from(data_dir_text);
    boot_log("assets.master.materialize");
    let master_data_path = match materialize_master(&data_dir, asset_manager) {
        Ok(path) => path,
        Err(error) => {
            boot_log(format!("fatal.master_data\t{error}"));
            return -7;
        }
    };
    boot_log("assets.master.ready");
    let capability = capability.to_ascii_lowercase();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (control_tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel();
    let active_usn = Arc::new(AtomicI64::new(0));
    let server_active_usn = Arc::clone(&active_usn);
    #[cfg(target_os = "android")]
    let asset_source = Some(Arc::new(AndroidAssetSource {
        manager: asset_manager as usize,
    }) as Arc<dyn AssetSource>);
    #[cfg(not(target_os = "android"))]
    let asset_source = None;
    let thread = std::thread::Builder::new()
        .name("ggfm-server".into())
        .spawn({
            let capability = capability.clone();
            move || {
                boot_log("runtime.tokio.start");
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("embedded runtime");
                runtime.block_on(async move {
                    match ggfm_transport_http::start(ggfm_transport_http::ServerConfig {
                        database_path: data_dir.join("ggfm.sqlite3"),
                        master_data_path,
                        capability,
                        clock,
                        asset_source,
                        progress: Some(Arc::new(|message: &str| boot_log(message))),
                    })
                    .await
                    {
                        Ok(mut handle) => {
                            boot_log("server.handle.ready");
                            let _ = ready_tx.send(Ok(handle.endpoint.clone()));
                            while let Some(command) = control_rx.recv().await {
                                match command {
                                    Control::BeginLogin { clock, reply } => {
                                        boot_log("identity.login.begin");
                                        let result =
                                            handle.begin_login(clock).await.map(|session| {
                                                server_active_usn
                                                    .store(session.identity.usn, Ordering::Release);
                                                boot_log("identity.login.ready");
                                            });
                                        let _ = reply.send(result);
                                    }
                                    Control::PrepareShutdown { now, reply } => {
                                        let _ = reply.send(handle.prepare_shutdown(now).await);
                                    }
                                    Control::Stop => {
                                        let _ = handle.prepare_shutdown(unix_now()).await;
                                        handle.shutdown();
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            boot_log(format!("fatal.server_start\t{error}"));
                            let _ = ready_tx.send(Err(error));
                        }
                    }
                });
            }
        });
    let Ok(thread) = thread else {
        boot_log("fatal.server_thread");
        return -5;
    };
    let endpoint = match ready_rx.recv() {
        Ok(Ok(endpoint)) => endpoint,
        Ok(Err(_)) | Err(_) => {
            let _ = thread.join();
            return -6;
        }
    };
    boot_log("transport.endpoint.published");
    *guard = Some(RuntimeState {
        endpoint: CString::new(endpoint).unwrap(),
        control: control_tx,
        thread: Some(thread),
        active_usn,
        _asset_manager: asset_manager as usize,
    });
    0
}

/// Return the USN selected by the most recent successful cold-login.
/// Zero means no login has completed in this process.
#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_active_usn() -> i64 {
    state()
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, |server| server.active_usn.load(Ordering::Acquire))
}

/// Copies pending diagnostic lines into a caller-owned UTF-8 buffer.
///
/// # Safety
///
/// When `capacity` is non-zero, `buffer` must point to at least `capacity`
/// writable bytes. The pointer must remain valid for the duration of this
/// call and must not alias server-owned memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ggfm_server_drain_logs(buffer: *mut c_char, capacity: usize) -> usize {
    if buffer.is_null() || capacity < 2 {
        return 0;
    }
    let mut logs = startup_logs().lock().unwrap();
    let mut bytes = Vec::new();
    while let Some(line) = logs.front() {
        let required = line.len() + usize::from(!bytes.is_empty());
        if bytes.len() + required >= capacity {
            break;
        }
        if !bytes.is_empty() {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(line.as_bytes());
        logs.pop_front();
    }
    if !bytes.is_empty() {
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
            *buffer.add(bytes.len()) = 0;
        }
    } else {
        unsafe { *buffer = 0 };
    }
    bytes.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_endpoint() -> *const c_char {
    state()
        .lock()
        .unwrap()
        .as_ref()
        .map_or(std::ptr::null(), |server| server.endpoint.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_begin_login(unix_seconds: i64, utc_offset_minutes: i32) -> i32 {
    let Ok(clock) = DeviceClock::new(unix_seconds, utc_offset_minutes) else {
        return -2;
    };
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let guard = state().lock().unwrap();
    let Some(server) = guard.as_ref() else {
        return -1;
    };
    if server
        .control
        .send(Control::BeginLogin {
            clock,
            reply: reply_tx,
        })
        .is_err()
    {
        return -3;
    }
    drop(guard);
    match reply_rx.recv() {
        Ok(Ok(())) => 0,
        _ => -4,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_prepare_shutdown() -> i32 {
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let guard = state().lock().unwrap();
    let Some(server) = guard.as_ref() else {
        return -1;
    };
    if server
        .control
        .send(Control::PrepareShutdown {
            now: unix_now(),
            reply: reply_tx,
        })
        .is_err()
    {
        return -2;
    }
    drop(guard);
    match reply_rx.recv() {
        Ok(Ok(())) => 0,
        _ => -3,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ggfm_server_stop() -> i32 {
    let mut runtime = state().lock().unwrap().take();
    if let Some(ref mut server) = runtime {
        let _ = server.control.send(Control::Stop);
        if let Some(thread) = server.thread.take() {
            let _ = thread.join();
        }
    }
    0
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(target_os = "android")]
fn materialize_master(data_dir: &std::path::Path, manager: *mut c_void) -> Result<PathBuf, String> {
    if manager.is_null() {
        return Err("Android AssetManager is null".into());
    }
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let asset_name = CString::new("ggfm/master.sqlite").unwrap();
    let asset = unsafe { AAssetManager_open(manager, asset_name.as_ptr(), 2) };
    if asset.is_null() {
        return Err("assets/ggfm/master.sqlite is missing".into());
    }
    struct Asset(*mut c_void);
    impl Drop for Asset {
        fn drop(&mut self) {
            unsafe { AAsset_close(self.0) };
        }
    }
    let asset = Asset(asset);
    let output = data_dir.join("master.sqlite");
    let temporary = data_dir.join(format!(".master.sqlite.{}.new", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let copy_result = (|| -> Result<(), String> {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read =
                unsafe { AAsset_read(asset.0, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
            if read < 0 {
                return Err("failed reading assets/ggfm/master.sqlite".into());
            }
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read as usize])
                .map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&temporary, &output).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if copy_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    copy_result.map(|()| output)
}

#[cfg(not(target_os = "android"))]
fn materialize_master(
    data_dir: &std::path::Path,
    _manager: *mut c_void,
) -> Result<PathBuf, String> {
    let path = data_dir.join("master.sqlite");
    path.is_file()
        .then_some(path)
        .ok_or_else(|| "master.sqlite is missing from dataDir".into())
}

#[cfg(target_os = "android")]
#[link(name = "android")]
unsafe extern "C" {
    fn AAssetManager_open(manager: *mut c_void, filename: *const c_char, mode: i32) -> *mut c_void;
    fn AAsset_read(asset: *mut c_void, buffer: *mut c_void, count: usize) -> i32;
    fn AAsset_getLength64(asset: *mut c_void) -> i64;
    fn AAsset_close(asset: *mut c_void);
}
