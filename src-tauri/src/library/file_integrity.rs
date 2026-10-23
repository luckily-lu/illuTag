//! File identity and library binding checks for recycle operations.
use super::*;
use rusqlite::OpenFlags;
use std::{fs::File, time::UNIX_EPOCH};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSignature {
    pub file_size: u64,
    pub modified_ns: i64,
    pub identity: String,
}

pub fn source_signature(path: &Path) -> Result<SourceSignature, String> {
    let file = File::open(path).map_err(|e| format!("read_error: {e}"))?;
    signature_from_file(&file)
}

fn signature_from_file(file: &File) -> Result<SourceSignature, String> {
    let metadata = file.metadata().map_err(|e| format!("read_error: {e}"))?;
    if !metadata.is_file() { return Err("not_regular_file".into()); }
    let modified = metadata.modified().map_err(|e| format!("read_error: {e}"))?;
    let modified_ns = match modified.duration_since(UNIX_EPOCH) {
        Ok(value) => value.as_nanos().min(i64::MAX as u128) as i64,
        Err(value) => -(value.duration().as_nanos().min(i64::MAX as u128) as i64),
    };
    Ok(SourceSignature { file_size: metadata.len(), modified_ns, identity: file_identity(file, &metadata)? })
}

#[cfg(windows)]
fn file_identity(file: &File, _: &std::fs::Metadata) -> Result<String, String> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32, creation: FileTime, access: FileTime, write: FileTime,
        volume_serial: u32, size_high: u32, size_low: u32, links: u32,
        index_high: u32, index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut ByHandleFileInformation) -> i32;
    }
    let mut info = ByHandleFileInformation::default();
    // SAFETY: the live File owns the handle; info is the documented, fully sized C structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!("read_error: {}", std::io::Error::last_os_error()));
    }
    Ok(format!("win:{:08x}:{:08x}{:08x}", info.volume_serial, info.index_high, info.index_low))
}

#[cfg(unix)]
fn file_identity(_: &File, metadata: &std::fs::Metadata) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn file_identity(_: &File, _: &std::fs::Metadata) -> Result<String, String> {
    Err("unsupported_file_identity".into())
}

pub(crate) fn db_binding_for(database_path: &Path) -> Result<String, String> {
    let info = source_signature(database_path)?;
    let canonical = fs::canonicalize(database_path).map_err(|e| e.to_string())?;
    Ok(format!("{}|{}", canonical.to_string_lossy(), info.identity))
}
pub(crate) fn db_binding(state: &AppState) -> Result<String, String> {
    db_binding_for(&state.database_path)
}
pub(crate) fn library(state: &AppState) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(&state.database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法读取图库：{e}"))?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-4096; PRAGMA mmap_size=0;")
        .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 3 {
        return Err("回收站需要 schema 3 图库".into());
    }
    Ok(conn)
}
