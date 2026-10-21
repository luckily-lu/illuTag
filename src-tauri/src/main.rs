#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use illutag_core::library::{get_image_detail, AppState};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{
    Manager, PhysicalPosition, PhysicalSize, Position, Size, State, WebviewWindow, WindowEvent,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DataDirectoryInfo {
    data_dir: String,
    using_fallback: bool,
    fallback_message: Option<String>,
    legacy_data_dir: Option<String>,
    legacy_database_detected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PortableConfig {
    data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DataDirectoryMigrationResult {
    source_dir: String,
    target_dir: String,
    config_path: String,
    copied_files: u64,
    copied_bytes: u64,
    operation_logs: Vec<illutag_core::library::OperationLogMigration>,
    restart_required: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowMemoryState {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    is_maximized: bool,
    is_fullscreen: bool,
}

const WINDOW_MEMORY_FILE_NAME: &str = "window-state.json";

const PORTABLE_CONFIG_FILE_NAME: &str = "illutag.portable.json";
const WINDOW_STATE_SAVE_DEBOUNCE_MS: u64 = 350;

fn capture_window_memory_state(window: &WebviewWindow) -> Result<WindowMemoryState, String> {
    let position = window
        .outer_position()
        .map_err(|error| format!("failed to read window position: {error}"))?;
    let size = window
        .inner_size()
        .map_err(|error| format!("failed to read window size: {error}"))?;
    if size.width < 500 || size.height < 500 || window.is_minimized().map_err(|e|e.to_string())? {
        return Err("跳过最小化或关闭期间的无效窗口尺寸".into());
    }
    let is_maximized = window
        .is_maximized()
        .map_err(|error| format!("failed to read window maximized state: {error}"))?;
    let is_fullscreen = window
        .is_fullscreen()
        .map_err(|error| format!("failed to read window fullscreen state: {error}"))?;

    Ok(WindowMemoryState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        is_maximized,
        is_fullscreen,
    })
}

fn window_state_intersects_available_monitor(
    window: &WebviewWindow,
    state: &WindowMemoryState,
) -> Result<bool, String> {
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("failed to read available monitors: {error}"))?;
    if monitors.is_empty() {
        return Ok(true);
    }

    let window_left = state.x as i64;
    let window_top = state.y as i64;
    let window_right = window_left + state.width as i64;
    let window_bottom = window_top + state.height as i64;

    Ok(monitors.iter().any(|monitor| {
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        let monitor_left = monitor_position.x as i64;
        let monitor_top = monitor_position.y as i64;
        let monitor_right = monitor_left + monitor_size.width as i64;
        let monitor_bottom = monitor_top + monitor_size.height as i64;
        let intersection_width = window_right.min(monitor_right) - window_left.max(monitor_left);
        let intersection_height = window_bottom.min(monitor_bottom) - window_top.max(monitor_top);
        intersection_width > 80 && intersection_height > 80
    }))
}

fn move_window_state_to_visible_monitor(
    window: &WebviewWindow,
    state: &mut WindowMemoryState,
) -> Result<(), String> {
    if window_state_intersects_available_monitor(window, state)? {
        return Ok(());
    }

    let monitors = window
        .available_monitors()
        .map_err(|error| format!("failed to read available monitors: {error}"))?;
    let Some(monitor) = monitors.first() else {
        return Ok(());
    };

    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let width = state.width.min(monitor_size.width).max(500);
    let height = state.height.min(monitor_size.height).max(500);

    state.width = width;
    state.height = height;
    state.x = monitor_position.x + ((monitor_size.width.saturating_sub(width) / 2) as i32);
    state.y = monitor_position.y + ((monitor_size.height.saturating_sub(height) / 2) as i32);
    Ok(())
}

fn restore_window_memory_state(
    window: &WebviewWindow,
    mut state: WindowMemoryState,
) -> Result<(), String> {
    state.width = state.width.max(500);
    state.height = state.height.max(500);
    move_window_state_to_visible_monitor(window, &mut state)?;

    if window
        .is_fullscreen()
        .map_err(|error| format!("failed to read window fullscreen state: {error}"))?
    {
        window
            .set_fullscreen(false)
            .map_err(|error| format!("failed to leave fullscreen before restore: {error}"))?;
    }

    if window
        .is_maximized()
        .map_err(|error| format!("failed to read window maximized state: {error}"))?
    {
        window
            .unmaximize()
            .map_err(|error| format!("failed to unmaximize before restore: {error}"))?;
    }

    window
        .set_size(Size::Physical(PhysicalSize {
            width: state.width,
            height: state.height,
        }))
        .map_err(|error| format!("failed to restore window size: {error}"))?;
    window
        .set_position(Position::Physical(PhysicalPosition {
            x: state.x,
            y: state.y,
        }))
        .map_err(|error| format!("failed to restore window position: {error}"))?;

    if state.is_maximized {
        window
            .maximize()
            .map_err(|error| format!("failed to restore window maximized state: {error}"))?;
    }
    if state.is_fullscreen {
        window
            .set_fullscreen(true)
            .map_err(|error| format!("failed to restore window fullscreen state: {error}"))?;
    }

    Ok(())
}

fn read_window_memory_state(path: &Path) -> Result<Option<WindowMemoryState>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read window memory state: {error}"))?;
    let state = serde_json::from_str::<WindowMemoryState>(&content)
        .map_err(|error| format!("failed to parse window memory state: {error}"))?;
    Ok(Some(state))
}

fn save_window_memory_state(path: &Path, window: &WebviewWindow) -> Result<(), String> {
    let state = capture_window_memory_state(window)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create window memory directory: {error}"))?;
    }
    let content = serde_json::to_string_pretty(&state)
        .map_err(|error| format!("failed to serialize window memory state: {error}"))?;
    fs::write(path, content)
        .map_err(|error| format!("failed to write window memory state: {error}"))
}

fn restore_window_memory_state_from_path(
    window: &WebviewWindow,
    path: &Path,
) -> Result<(), String> {
    if let Some(state) = read_window_memory_state(path)? {
        restore_window_memory_state(window, state)?;
    }
    Ok(())
}

fn install_window_memory_state_listener(window: &WebviewWindow, path: PathBuf) {
    let save_generation = Arc::new(AtomicU64::new(0));
    let window_for_listener = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            let generation = save_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let save_generation = Arc::clone(&save_generation);
            let window = window_for_listener.clone();
            let path = path.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(WINDOW_STATE_SAVE_DEBOUNCE_MS));
                if save_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Err(error) = save_window_memory_state(&path, &window) {
                    eprintln!("[window-memory] save failed: {error}");
                }
            });
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            if let Err(error) = save_window_memory_state(&path, &window_for_listener) {
                eprintln!("[window-memory] save failed: {error}");
            }
        }
        _ => {}
    });
}

#[tauri::command]
fn window_close_command(window: tauri::Window) -> Result<(), String> {
    window
        .close()
        .map_err(|error| format!("关闭窗口失败：{error}"))
}

#[tauri::command]
fn window_start_dragging_command(window: tauri::Window) -> Result<(), String> {
    window
        .start_dragging()
        .map_err(|error| format!("failed to start dragging window: {error}"))
}

#[tauri::command]
async fn open_gallery_image_with_default_app_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    commands::off_main(app, move |state| {
        open_path_with_default_app(&get_image_detail(image_id, &state)?.image.path)
    })
    .await
}

#[tauri::command]
fn open_external_url_command(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let allowed = trimmed.starts_with("https://github.com/YazawaSunrise/illuTag")
        || trimmed.starts_with("http://github.com/YazawaSunrise/illuTag");
    if !allowed {
        return Err("Unsupported external URL".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(trimmed)
            .spawn()
            .map_err(|error| format!("Failed to open URL: {error}"))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(trimmed)
            .status()
            .map_err(|error| format!("Failed to open URL: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to open URL. status={status}"))
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let status = Command::new("xdg-open")
            .arg(trimmed)
            .status()
            .map_err(|error| format!("Failed to open URL: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to open URL. status={status}"))
        }
    }
}

#[tauri::command]
fn open_data_directory_command(state: State<AppState>) -> Result<(), String> {
    open_path_with_default_app(&state.data_dir.to_string_lossy())
}

fn open_path_with_default_app(path: &str) -> Result<(), String> {
    let normalized_path = path.strip_prefix(r"\\?\").unwrap_or(path);

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(normalized_path)
            .spawn()
            .map_err(|error| format!("Failed to open image with default app: {error}"))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(normalized_path)
            .status()
            .map_err(|error| format!("Failed to open image with default app: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "Failed to open image with default app. status={status}"
            ))
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let status = Command::new("xdg-open")
            .arg(normalized_path)
            .status()
            .map_err(|error| format!("Failed to open image with default app: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "Failed to open image with default app. status={status}"
            ))
        }
    }
}

fn exe_dir() -> Result<PathBuf, String> {
    let exe_path =
        env::current_exe().map_err(|error| format!("Failed to resolve exe path: {error}"))?;
    exe_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Failed to resolve exe directory".to_string())
}

fn portable_config_path() -> Result<PathBuf, String> {
    Ok(exe_dir()?.join(PORTABLE_CONFIG_FILE_NAME))
}

fn read_portable_config(path: &Path) -> Result<PortableConfig, String> {
    if !path.is_file() {
        return Ok(PortableConfig::default());
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read portable config {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "Failed to parse portable config {}: {error}",
            path.display()
        )
    })
}

fn write_portable_config(path: &Path, config: &PortableConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create portable config directory: {error}"))?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| format!("Failed to serialize portable config: {error}"))?;
    illutag_core::runtime::write_atomic(path, text.as_bytes())
}

fn configured_portable_data_dir(
    config: &PortableConfig,
    executable_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(value) = config
        .data_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let configured = PathBuf::from(value);
    let resolved = if configured.is_absolute() {
        configured
    } else {
        executable_dir.join(configured)
    };
    if !resolved.is_absolute() {
        return Err("便携数据目录不能使用依赖当前工作目录的驱动器相对路径".into());
    }
    Ok(Some(resolved))
}

#[tauri::command]
async fn migrate_data_directory_command(
    target_dir: String,
    app: tauri::AppHandle,
) -> Result<DataDirectoryMigrationResult, String> {
    commands::off_main(app, move |state| {
        let target = PathBuf::from(target_dir);
        let config = portable_config_path()?;
        let (copied_files, copied_bytes, operation_logs) =
            illutag_core::library::prepare_data_directory_migration(&target, &state)?;
        if let Err(error) = write_portable_config(
            &config,
            &PortableConfig {
                data_dir: Some(target.to_string_lossy().into()),
            },
        ) {
            state
                .maintenance
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(error);
        }
        let unresolved: usize = operation_logs.iter().map(|log| log.unresolved.len()).sum();
        let message = if unresolved == 0 {
            "数据目录已校验并切换，请重启应用。原目录保持完整。".to_string()
        } else {
            format!(
                "数据目录已校验并切换，请重启应用。有 {unresolved} 条旧操作记录未能迁移，详情见提示列表。"
            )
        };
        Ok(DataDirectoryMigrationResult {
            source_dir: state.data_dir.to_string_lossy().into(),
            target_dir: target.to_string_lossy().into(),
            config_path: config.to_string_lossy().into(),
            copied_files,
            copied_bytes,
            operation_logs,
            restart_required: true,
            message,
        })
    })
    .await
}

fn resolve_portable_data_dir(system_app_data_dir: PathBuf) -> (PathBuf, Option<String>) {
    if let Ok(config_path) = portable_config_path() {
        match read_portable_config(&config_path) {
            Ok(config) => {
                let configured_dir = config_path
                    .parent()
                    .ok_or_else(|| "无法定位便携配置目录".to_string())
                    .and_then(|dir| configured_portable_data_dir(&config, dir));
                if let Ok(Some(configured_data_dir)) = configured_dir.as_ref() {
                    match fs::create_dir_all(configured_data_dir) {
                        Ok(()) => return (configured_data_dir.clone(), None),
                        Err(error) => {
                            let message = format!(
                                "配置的数据目录 {} 无法创建：{error}。已回退到默认数据目录。",
                                configured_data_dir.display()
                            );
                            eprintln!("[data-dir] {message}");
                        }
                    }
                } else if let Err(error) = configured_dir {
                    eprintln!("[data-dir] {error}");
                }
            }
            Err(error) => eprintln!("[data-dir] {error}"),
        }
    }

    let exe_data_dir_result = exe_dir().map(|dir| dir.join("data"));

    match exe_data_dir_result {
        Ok(data_dir) => match fs::create_dir_all(&data_dir) {
            Ok(()) => {
                let portable_database_exists = data_dir.join("illutag.sqlite").is_file();
                let legacy_database_exists = system_app_data_dir.join("illutag.sqlite").is_file();
                if !portable_database_exists && legacy_database_exists {
                    let message = format!(
                        "检测到旧版 AppData 数据库，当前暂时使用旧数据目录 {}。可在设置中迁移数据目录。",
                        system_app_data_dir.display()
                    );
                    eprintln!("[data-dir] {message}");
                    return (system_app_data_dir, Some(message));
                }
                (data_dir, None)
            }
            Err(error) => {
                let message = format!(
                    "无法创建 exe 同级 data 目录 {}：{error}。已改用系统数据目录 {}",
                    data_dir.display(),
                    system_app_data_dir.display()
                );
                eprintln!("[data-dir] {message}");
                if let Err(fallback_error) = fs::create_dir_all(&system_app_data_dir) {
                    eprintln!(
                        "[data-dir] failed to create fallback data directory {}: {fallback_error}",
                        system_app_data_dir.display()
                    );
                }
                (system_app_data_dir, Some(message))
            }
        },
        Err(error) => {
            let message = format!(
                "无法定位 exe 数据目录：{error}。已改用系统数据目录 {}",
                system_app_data_dir.display()
            );
            eprintln!("[data-dir] {message}");
            if let Err(fallback_error) = fs::create_dir_all(&system_app_data_dir) {
                eprintln!(
                    "[data-dir] failed to create fallback data directory {}: {fallback_error}",
                    system_app_data_dir.display()
                );
            }
            (system_app_data_dir, Some(message))
        }
    }
}

mod commands;
struct DataLease {
    _file: fs::File,
}
fn acquire_data_lease(dir: &Path) -> Result<DataLease, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
        .open(dir.join("illutag.session.lock"))
        .map(|file| DataLease { _file: file })
        .map_err(|e| format!("此图库已由另一个实例使用：{e}"))
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let setup_started = Instant::now();
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| Box::<dyn std::error::Error>::from(error))?;
            let legacy_data_dir = app_data_dir.clone();
            let isolated = illutag_core::runtime::RuntimePaths::from_env()?;
            let (data_dir, data_dir_fallback_message) = match isolated.data_root {
                Some(path) => (path, None),
                None => resolve_portable_data_dir(app_data_dir),
            };
            let mut window_builder =
                tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?;
            if let Some(profile) = isolated.profile_root {
                window_builder = window_builder.data_directory(profile);
            }
            let main_window = window_builder.build()?;
            let _data_lease = acquire_data_lease(&data_dir)?;
            app.manage(_data_lease);
            illutag_core::library::compact::check_pending_switch(&data_dir)?;
            let window_memory_state_path = data_dir.join(WINDOW_MEMORY_FILE_NAME);

            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) =
                    restore_window_memory_state_from_path(&window, &window_memory_state_path)
                {
                    eprintln!("[window-memory] restore failed: {error}");
                }
                install_window_memory_state_listener(&window, window_memory_state_path);
            }
            eprintln!(
                "[startup-prof] setup_window_shown_ms={}",
                setup_started.elapsed().as_millis()
            );

            let database_file_name = if env::var("ILLUTAG_USE_TEST_DB")
                .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false)
            {
                "illutag.test.sqlite"
            } else {
                "illutag.sqlite"
            };
            let database_path = data_dir.join(database_file_name);
            eprintln!("[database] using {}", database_path.display());
            let app_state = AppState::new(
                database_path,
                data_dir,
                data_dir_fallback_message,
                Some(legacy_data_dir),
            );
            // P0: 启动路径不再同步预热点 CLIP 向量缓存（809K×512 f32 ≈ 1.66GB）。
            // 首次语义/自然语言/以图搜图时由 ensure_clip_vector_cache_loaded 惰性加载，
            // 避免该全量读压在首帧与 list_library 之前。回滚：恢复此处的 warmup_clip_vector_cache 调用。
            app_state.jobs.hydrate(&app_state)?;
            app.manage(app_state);
            main_window.show()?;
            eprintln!(
                "[startup-prof] setup_total_ms={}",
                setup_started.elapsed().as_millis()
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_library,
            commands::data_directory_info_command,
            migrate_data_directory_command,
            commands::list_gallery_images_page_command,
            commands::list_gallery_image_ids_for_scope_command,
            commands::list_gallery_images_by_ids_page_command,
            commands::dev_create_fake_gallery_data_command,
            commands::dev_create_small_file_test_set_command,
            commands::dev_cleanup_stress_test_data_command,
            commands::add_gallery_folder_command,
            commands::remove_gallery_folder_command,
            commands::remove_image_from_index_command,
            commands::remove_images_from_index_command,
            commands::restore_image_from_trash_command,
            commands::restore_images_from_trash_command,
            commands::move_image_to_system_trash_command,
            commands::move_images_to_system_trash_command,
            commands::toggle_image_favorite_command,
            commands::set_images_favorite_command,
            commands::read_image_bytes_command,
            commands::copy_image_to_system_clipboard_command,
            commands::test_wd_swinv2_tagger_command,
            commands::start_scan_all_folders_with_tagging_command,
            commands::start_scan_all_folders_collect_only_command,
            commands::start_tag_pending_images_only_command,
            commands::background_scan_status_command,
            commands::background_scan_progress_command,
            commands::pause_background_scan_command,
            commands::resume_background_scan_command,
            commands::stop_background_scan_command,
            commands::start_natural_language_scan_command,
            commands::natural_language_scan_status_command,
            commands::natural_language_scan_progress_command,
            commands::pause_natural_language_scan_command,
            commands::resume_natural_language_scan_command,
            commands::stop_natural_language_scan_command,
            commands::start_startup_cleanup_command,
            commands::startup_cleanup_status_command,
            commands::start_thumbnail_generation_command,
            commands::thumbnail_generation_status_command,
            commands::pause_thumbnail_generation_command,
            commands::resume_thumbnail_generation_command,
            commands::stop_thumbnail_generation_command,
            commands::clear_thumbnail_cache_command,
            commands::rebuild_thumbnail_cache_command,
            commands::list_image_auto_tags_command,
            commands::list_image_user_tags_command,
            commands::add_image_user_custom_tag_command,
            commands::remove_image_user_custom_tag_command,
            commands::add_image_user_supplement_tag_command,
            commands::remove_image_user_supplement_tag_command,
            commands::list_tag_management_state_command,
            commands::list_tag_dictionary_browser_command,
            commands::list_tag_covers_command,
            commands::save_tag_cover_command,
            commands::query_tag_preview_command,
            commands::query_tag_cover_candidates_command,
            commands::export_data_migration_backup_command,
            commands::inspect_data_migration_backup_command,
            commands::import_data_migration_backup_command,
            commands::export_organized_folder_result_command,
            commands::create_user_tag_folder_command,
            commands::create_user_custom_tag_command,
            commands::delete_user_custom_tag_command,
            commands::rename_user_tag_folder_command,
            commands::delete_user_tag_folder_command,
            commands::assign_user_tag_to_folder_command,
            commands::unassign_user_tag_from_folder_command,
            commands::suggest_known_auto_tags_command,
            commands::resolve_tag_labels_command,
            commands::search_gallery_image_ids_command,
            commands::search_gallery_image_ids_by_natural_language_command,
            commands::create_user_folder_command,
            commands::rename_user_folder_command,
            commands::delete_user_folder_command,
            commands::reorder_user_folder_command,
            commands::get_user_folder_rule_command,
            commands::save_user_folder_rule_command,
            commands::assign_image_to_user_folder_command,
            commands::assign_images_to_user_folder_command,
            commands::remove_image_from_user_folder_command,
            commands::move_images_to_user_folder_command,
            commands::remove_images_from_user_folder_command,
            commands::add_images_user_tags_command,
            commands::create_reference_board_folder_command,
            commands::create_reference_board_command,
            commands::add_image_to_reference_board_command,
            commands::paste_image_to_reference_board_command,
            commands::paste_images_to_reference_board_command,
            commands::duplicate_reference_board_item_command,
            commands::restore_reference_board_item_command,
            commands::import_reference_board_item_to_library_command,
            commands::export_reference_board_item_command,
            commands::export_gallery_image_command,
            commands::rename_reference_board_command,
            commands::rename_reference_board_folder_command,
            commands::reorder_reference_board_folder_command,
            commands::move_reference_board_to_folder_command,
            commands::reorder_reference_board_command,
            commands::delete_reference_board_command,
            commands::delete_reference_board_folder_command,
            commands::remove_reference_board_item_command,
            commands::update_reference_board_item_layout_command,
            commands::bring_reference_board_item_to_front_command,
            commands::auto_arrange_reference_board_command,
            commands::window_minimize_command,
            commands::window_toggle_maximize_command,
            commands::window_is_maximized_command,
            commands::window_toggle_always_on_top_command,
            commands::window_is_always_on_top_command,
            window_start_dragging_command,
            open_gallery_image_with_default_app_command,
            open_data_directory_command,
            open_external_url_command,
            window_close_command,
            commands::get_bootstrap,
            commands::get_navigation,
            commands::query_gallery,
            commands::get_image_detail,
            commands::mutate_images,
            commands::undo_viewer_trash,
            commands::list_favorite_tags,
            commands::set_favorite_tag,
            commands::start_job,
            commands::control_job,
            commands::list_jobs,
            commands::job_history,
            commands::retry_job,
            commands::clear_job_history,
            commands::duplicate_status,
            commands::duplicate_workflow_status,
            commands::query_duplicate_workflow,
            commands::edit_duplicate_review,
            commands::prepare_duplicate_workflow,
            commands::query_duplicate_groups,
            commands::query_duplicate_members,
            commands::clear_duplicate_cache,
            commands::query_duplicate_group,
            commands::prepare_duplicate_action,
            commands::apply_duplicate_action,
            commands::prepare_recycle_selection,
            commands::apply_recycle_action,
            commands::recycle_operation_status,
            commands::recycle_operation_results,
            commands::duplicate_operation_status,
            commands::duplicate_operation_results,
            commands::undo_duplicate_action,
            commands::clear_duplicate_management_records,
            commands::create_restore_package,
            commands::inspect_restore_package,
            commands::restore_package,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run illuTag");
}

#[cfg(test)]
mod portable_path_tests {
    use super::*;

    #[test]
    fn relative_data_directory_is_anchored_to_executable() {
        let executable_dir = env::temp_dir().join("illutag-portable-app");
        let config = PortableConfig {
            data_dir: Some(" data ".into()),
        };
        let expected = executable_dir.join("data");
        assert_eq!(
            configured_portable_data_dir(&config, &executable_dir).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn absolute_data_directory_is_preserved() {
        let data_dir = env::temp_dir().join("illutag-existing-library");
        let config = PortableConfig {
            data_dir: Some(data_dir.to_string_lossy().into()),
        };
        assert_eq!(
            configured_portable_data_dir(&config, &env::temp_dir()).unwrap(),
            Some(data_dir)
        );
        #[cfg(windows)]
        {
            let production = PathBuf::from(r"\\?\D:\Ai\illuTag-danbooru-tag-query");
            let config = PortableConfig {
                data_dir: Some(production.to_string_lossy().into()),
            };
            assert_eq!(
                configured_portable_data_dir(&config, &env::temp_dir()).unwrap(),
                Some(production)
            );
        }
    }

    #[test]
    fn empty_and_unknown_configuration_keeps_default_resolution() {
        for json in [r"{}", r#"{"dataDir":" "}"#, r#"{"unknown":"data"}"#] {
            let config = serde_json::from_str(json).unwrap();
            assert_eq!(
                configured_portable_data_dir(&config, &env::temp_dir()).unwrap(),
                None
            );
        }
    }

    #[test]
    fn shortcut_working_directory_cannot_redirect_portable_data() {
        struct RestoreWorkingDirectory(PathBuf);
        impl Drop for RestoreWorkingDirectory {
            fn drop(&mut self) {
                env::set_current_dir(&self.0).expect("restore test working directory");
            }
        }
        let _restore = RestoreWorkingDirectory(env::current_dir().unwrap());
        let executable_dir = env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let config = PortableConfig {
            data_dir: Some("data".into()),
        };
        let before = configured_portable_data_dir(&config, &executable_dir).unwrap();
        env::set_current_dir(env::temp_dir()).unwrap();
        let after = configured_portable_data_dir(&config, &executable_dir).unwrap();
        assert_eq!(after, before);
        assert_eq!(after, Some(executable_dir.join("data")));
        #[cfg(windows)]
        assert!(configured_portable_data_dir(
            &PortableConfig {
                data_dir: Some("D:relative".into())
            },
            &executable_dir,
        )
        .is_err());
    }
}
