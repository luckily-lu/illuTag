//! Explicit isolation for development, benchmarks and recovery rehearsals.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub data_root: Option<PathBuf>,
    pub profile_root: Option<PathBuf>,
}
impl RuntimePaths {
    pub fn from_env() -> Result<Self, String> {
        let data_root = env::var_os("ILLUTAG_DATA_ROOT").map(PathBuf::from);
        let profile_root = env::var_os("ILLUTAG_PROFILE_ROOT").map(PathBuf::from);
        if data_root.is_some() {
            for name in [
                "ILLUTAG_DATA_ROOT",
                "ILLUTAG_PROFILE_ROOT",
                "ILLUTAG_ASSET_ROOT",
                "ILLUTAG_LOG_ROOT",
            ] {
                let path = env::var_os(name)
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("隔离模式必须设置 {name}"))?;
                if !path.is_absolute() {
                    return Err(format!("{name} 必须是绝对路径"));
                }
                fs::create_dir_all(&path).map_err(|e| format!("无法准备 {name}：{e}"))?;
            }
        } else if profile_root.is_some() {
            return Err("设置 profile 时必须同时指定独立数据目录".into());
        }
        Ok(Self {
            data_root,
            profile_root,
        })
    }
}

pub fn check_asset_write(path: &Path) -> Result<(), String> {
    if env::var("ILLUTAG_READ_ONLY").as_deref() == Ok("1") {
        return Err("只读图库禁止修改文件".into());
    }
    if let Some(root) = env::var_os("ILLUTAG_ASSET_ROOT") {
        let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        let target = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !target.starts_with(root) {
            return Err("测试操作超出隔离素材目录".into());
        }
    }
    Ok(())
}

/// The destination changes only after the complete sibling file has reached disk.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("配置文件必须有父目录")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let staged = parent.join(format!(
        ".illutag-config-{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staged, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result.map_err(|e| format!("配置切换失败，原配置保留：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_config_replaces_only_complete_file() {
        let root = env::temp_dir().join(format!("illutag-atomic-config-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("config.json");
        fs::write(&file, b"old").unwrap();
        write_atomic(&file, b"new").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        let blocked = root.join("directory.json");
        fs::create_dir_all(&blocked).unwrap();
        fs::write(blocked.join("original"), b"keep").unwrap();
        assert!(write_atomic(&blocked, b"replacement").is_err());
        assert_eq!(fs::read(blocked.join("original")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }
}
