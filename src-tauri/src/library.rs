use calamine::{open_workbook_auto, Data as ExcelCell, Reader};
use image::imageops::FilterType;
use image::GenericImageView;
use image::{ImageBuffer, ImageReader, Rgb};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, HashSet},
    env, fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, TrySendError},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

const WD_TAGGER_MODEL_NAME: &str = "wd-swinv2-tagger-v3";
const CHINESE_CLIP_MODEL_ID: &str = "cn_clip_vit_base_patch16";
const CHINESE_CLIP_MODEL_VERSION: &str = "onnx_v1";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn apply_hidden_child_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn resolve_python_executable_path() -> PathBuf {
    static PYTHON_EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    PYTHON_EXECUTABLE
        .get_or_init(|| {
            let mut candidates = Vec::<PathBuf>::new();
            if let Some(root) = env::var_os("ILLUTAG_RESOURCE_ROOT") {
                return PathBuf::from(root).join("runtime/python/python.exe");
            }
            if let Ok(exe_path) = env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    candidates.push(exe_dir.join("runtime").join("python").join("python.exe"));
                    candidates.push(
                        exe_dir
                            .join("..")
                            .join("runtime")
                            .join("python")
                            .join("python.exe"),
                    );
                    candidates.push(
                        exe_dir
                            .join("resources")
                            .join("runtime")
                            .join("python")
                            .join("python.exe"),
                    );
                }
            }
            if let Ok(cwd) = env::current_dir() {
                candidates.push(cwd.join("runtime").join("python").join("python.exe"));
                candidates.push(
                    cwd.join("src-tauri")
                        .join("runtime")
                        .join("python")
                        .join("python.exe"),
                );
            }
            for candidate in candidates {
                if candidate.is_file() {
                    eprintln!(
                        "[python-runtime] using embedded python: {}",
                        candidate.display()
                    );
                    return candidate;
                }
            }
            eprintln!("[python-runtime] using system python from PATH");
            PathBuf::from("python")
        })
        .clone()
}

fn python_command() -> Command {
    let mut command = Command::new(resolve_python_executable_path());
    apply_hidden_child_window(&mut command);
    command
}
const THUMBNAIL_LONG_EDGE: u32 = 960;
const THUMBNAIL_WEBP_QUALITY: f32 = 85.0;
const THUMBNAIL_WORKER_COUNT: usize = 3;
const THUMBNAIL_WORKER_QUEUE_CAPACITY: usize = 2;
const NATURAL_LANGUAGE_SEARCH_DEFAULT_TOP_K: usize = 600;
const CLIP_IMAGE_SERVICE_IDLE_RELEASE_MS: i64 = 20 * 60 * 1000;
const CLIP_IMAGE_SERVICE_IDLE_CHECK_INTERVAL_MS: u64 = 30_000;
const USER_FOLDER_SOURCE_KIND_LIBRARY_DIR: &str = "library_dir";
const TAG_DICTIONARY_SOURCE_SCHEMA_VERSION: &str = "csv-col2-col5-v1";
const BATCH_SQL_VARIABLE_LIMIT_SAFE: usize = 900;
// Bump when migrate_database() changes; gates the one-time schema setup via PRAGMA user_version.
const SCHEMA_USER_VERSION: i64 = 3;
const LARGE_LIBRARY_IMAGE_THRESHOLD: i64 = 30_000;
const LARGE_LIBRARY_INITIAL_IMAGE_LIMIT: i64 = 5_000;
const TAG_SAVE_BATCH_SIZE: usize = 100;
const ACTIVE_GALLERY_IMAGE_WHERE: &str =
    "COALESCE(i.source, '') <> 'reference' AND COALESCE(i.trashed, 0) = 0";

#[derive(Clone)]
pub struct AppState {
    pub database_path: PathBuf,
    pub write_lock: Arc<Mutex<()>>,
    pub jobs: Arc<crate::jobs::JobRuntime>,
    pub clip_load_lock: Arc<Mutex<()>>,
    pub read_only: bool,
    pub foreground_queries: Arc<std::sync::atomic::AtomicUsize>,
    pub scan_folder_id: Option<i64>,
    pub maintenance: Arc<std::sync::atomic::AtomicBool>,
    pub data_dir: PathBuf,
    pub data_dir_fallback_message: Option<String>,
    pub legacy_data_dir: Option<PathBuf>,
    pub library: Arc<Mutex<Option<LibraryStore>>>,
    pub background_scan_running: Arc<Mutex<bool>>,
    pub background_scan_pending: Arc<Mutex<bool>>,
    pub background_scan_pause_requested: Arc<Mutex<bool>>,
    pub background_scan_stop_requested: Arc<Mutex<bool>>,
    pub background_scan_progress: Arc<Mutex<BackgroundScanProgress>>,
    pub startup_cleanup_running: Arc<Mutex<bool>>,
    pub startup_cleanup_generation: Arc<Mutex<i64>>,
    pub startup_cleanup_error: Arc<Mutex<Option<String>>>,
    pub thumbnail_generation_running: Arc<Mutex<bool>>,
    pub thumbnail_generation_pending: Arc<Mutex<bool>>,
    pub thumbnail_generation_pause_requested: Arc<Mutex<bool>>,
    pub thumbnail_generation_stop_requested: Arc<Mutex<bool>>,
    pub thumbnail_generation_progress: Arc<Mutex<ThumbnailGenerationProgress>>,
    pub natural_language_scan_running: Arc<Mutex<bool>>,
    pub natural_language_scan_pending: Arc<Mutex<bool>>,
    pub natural_language_scan_pause_requested: Arc<Mutex<bool>>,
    pub natural_language_scan_stop_requested: Arc<Mutex<bool>>,
    pub natural_language_scan_progress: Arc<Mutex<NaturalLanguageScanProgress>>,
    pub clip_vector_cache: Arc<Mutex<Option<ClipImageVectorCache>>>,
    pub clip_text_encoder_service: Arc<Mutex<Option<ClipTextEncoderService>>>,
    pub clip_image_encoder_service: Arc<Mutex<Option<ClipImageEncoderService>>>,
    pub clip_image_encoder_last_used_at: Arc<Mutex<i64>>,
    pub clip_image_encoder_release_worker_running: Arc<Mutex<bool>>,
    pub wd_tagger_service: Arc<Mutex<Option<WdTaggerService>>>,
}

pub struct ClipImageVectorCache {
    model_id: String,
    model_version: String,
    dimension: usize,
    vectors: HashMap<String, Vec<f32>>,
}

pub struct ClipTextEncoderService {
    child: Child,
    stdin: ChildStdin,
    stdout: crate::protocol::ResponseReader,
    model_root: PathBuf,
}

pub struct ClipImageEncoderService {
    child: Child,
    stdin: ChildStdin,
    stdout: crate::protocol::ResponseReader,
    model_root: PathBuf,
}

pub struct WdTaggerService {
    child: Child,
    stdin: ChildStdin,
    stdout: crate::protocol::ResponseReader,
    model_path: PathBuf,
    tags_path: PathBuf,
}

mod contracts;
pub use contracts::*;

mod gallery;
pub use gallery::*;

mod workers;
pub use workers::*;

mod search;
pub use search::*;

mod tags;
pub use tags::*;

mod legacy_backup;
pub use legacy_backup::*;

mod tag_management;
pub use tag_management::*;

mod development;
pub use development::*;

mod tag_search;
pub use tag_search::*;

mod folders;
pub use folders::*;

mod legacy_boards;
pub use legacy_boards::*;

pub mod compact;

mod storage;
use storage::*;

mod scan;
use scan::*;

mod thumbnails;
use thumbnails::*;

mod embeddings;
use embeddings::*;

mod inference;
pub use inference::*;

mod api;
pub use api::*;
pub fn job_database(state: &AppState) -> Result<Connection, String> {
    open_database(&state.database_path)
}

mod recovery;
pub use recovery::*;

impl AppState {
    pub fn new(
        database_path: PathBuf,
        data_dir: PathBuf,
        data_dir_fallback_message: Option<String>,
        legacy_data_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            database_path,
            write_lock: Arc::new(Mutex::new(())),
            jobs: Arc::new(crate::jobs::JobRuntime::default()),
            clip_load_lock: Arc::new(Mutex::new(())),
            foreground_queries: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            scan_folder_id: None,
            maintenance: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            read_only: env::var("ILLUTAG_READ_ONLY").as_deref() == Ok("1"),
            data_dir,
            data_dir_fallback_message,
            legacy_data_dir,
            library: Arc::new(Mutex::new(None)),
            background_scan_running: Arc::new(Mutex::new(false)),
            background_scan_pending: Arc::new(Mutex::new(false)),
            background_scan_pause_requested: Arc::new(Mutex::new(false)),
            background_scan_stop_requested: Arc::new(Mutex::new(false)),
            background_scan_progress: Arc::new(Mutex::new(BackgroundScanProgress::default())),
            startup_cleanup_running: Arc::new(Mutex::new(false)),
            startup_cleanup_generation: Arc::new(Mutex::new(0)),
            startup_cleanup_error: Arc::new(Mutex::new(None)),
            thumbnail_generation_running: Arc::new(Mutex::new(false)),
            thumbnail_generation_pending: Arc::new(Mutex::new(false)),
            thumbnail_generation_pause_requested: Arc::new(Mutex::new(false)),
            thumbnail_generation_stop_requested: Arc::new(Mutex::new(false)),
            thumbnail_generation_progress: Arc::new(Mutex::new(
                ThumbnailGenerationProgress::default(),
            )),
            natural_language_scan_running: Arc::new(Mutex::new(false)),
            natural_language_scan_pending: Arc::new(Mutex::new(false)),
            natural_language_scan_pause_requested: Arc::new(Mutex::new(false)),
            natural_language_scan_stop_requested: Arc::new(Mutex::new(false)),
            natural_language_scan_progress: Arc::new(Mutex::new(
                NaturalLanguageScanProgress::default(),
            )),
            clip_vector_cache: Arc::new(Mutex::new(None)),
            clip_text_encoder_service: Arc::new(Mutex::new(None)),
            clip_image_encoder_service: Arc::new(Mutex::new(None)),
            clip_image_encoder_last_used_at: Arc::new(Mutex::new(0)),
            clip_image_encoder_release_worker_running: Arc::new(Mutex::new(false)),
            wd_tagger_service: Arc::new(Mutex::new(None)),
        }
    }
}

#[cfg(test)]
mod tests;
