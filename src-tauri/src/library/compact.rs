//! Schema 3 keeps public path IDs at the read boundary and uses integer keys on disk.
use super::*;

pub const SCHEMA: &str = include_str!("compact_schema.sql");
pub const INDEXES: &str = include_str!("compact_indexes.sql");

pub fn check_pending_switch(data_dir: &Path) -> Result<(), String> {
    let ledger = data_dir.join("illutag.storage-switch.json");
    if !ledger.exists() {
        return Ok(());
    }
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(ledger).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    match state.get("phase").and_then(|v|v.as_str()) {
        Some("accepted") => Ok(()),
        Some("newInstalled") if env::var("ILLUTAG_READ_ONLY").as_deref()==Ok("1") => Ok(()),
        _ => Err("存储升级尚未完成，原库已保留。请运行 storage_cutover.py recover 恢复，或完成只读验收后接受升级".into()),
    }
}

pub(super) fn physical_name(name: &str) -> String {
    match name {
        "image_auto_tags" => "storage_auto_tags".into(),
        "image_clip_embeddings" => "storage_clip_embeddings".into(),
        "image_tags"
        | "image_ai_state"
        | "image_user_folders"
        | "image_user_custom_tags"
        | "image_user_supplement_tags"
        | "image_thumbnails"
        | "image_atmosphere_signatures"
        | "image_color_signatures"
        | "reference_board_items" => format!("storage_{name}"),
        _ => name.to_string(),
    }
}

pub(super) fn initialize(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA page_size=16384; BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    let result = conn.execute_batch(&format!(
        "{SCHEMA}\n{INDEXES}\nPRAGMA user_version=3; COMMIT;"
    ));
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result.map_err(|e| e.to_string())
}

pub(super) fn intern(conn: &Connection, value: &str) -> Result<i64, String> {
    conn.prepare_cached(
        "INSERT INTO storage_strings(value) VALUES(?1) ON CONFLICT(value) DO NOTHING",
    )
    .map_err(|e| e.to_string())?
    .execute([value])
    .map_err(|e| e.to_string())?;
    conn.prepare_cached("SELECT string_pk FROM storage_strings WHERE value=?1")
        .map_err(|e| e.to_string())?
        .query_row([value], |r| r.get(0))
        .map_err(|e| e.to_string())
}

pub(super) fn upsert_auto_tag(
    conn: &Connection,
    image_id: &str,
    category: &str,
    tag_en: &str,
    tag_zh: Option<&str>,
    confidence: f64,
    model_name: &str,
    updated_at: i64,
) -> Result<(), String> {
    let model_pk = intern(conn, model_name)?;
    let category_pk = intern(conn, category)?;
    let tag_pk = intern(conn, tag_en)?;
    let translation_pk = tag_zh.map(|v| intern(conn, v)).transpose()?;
    conn.prepare_cached("INSERT INTO storage_auto_tags(image_pk,model_pk,category_pk,tag_pk,translation_pk,confidence,updated_at)
      VALUES((SELECT image_pk FROM images WHERE id=?1),?2,?3,?4,?5,?6,?7)
      ON CONFLICT(image_pk,model_pk,category_pk,tag_pk) DO UPDATE SET
      translation_pk=excluded.translation_pk,confidence=excluded.confidence,updated_at=excluded.updated_at")
        .map_err(|e| e.to_string())?
        .execute(params![image_id, model_pk, category_pk, tag_pk, translation_pk, confidence, updated_at])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn delete_auto_tags_for_image_path(
    conn: &Connection,
    image_path: &str,
) -> Result<(), String> {
    // The scan stage only owns the path. A mapped import/migration may keep a
    // legacy public id that no longer equals the path, so never match on id here.
    conn.execute(
        "DELETE FROM storage_auto_tags WHERE image_pk=(SELECT image_pk FROM images WHERE path=?1)",
        [image_path],
    )
    .map_err(|error| format!("Failed to invalidate stale auto tags: {error}"))?;
    Ok(())
}

pub(super) fn clip_model(conn: &Connection, id: &str, version: &str) -> Result<i64, String> {
    conn.prepare_cached(
        "INSERT INTO storage_clip_models(model_id,model_version) VALUES(?1,?2)
      ON CONFLICT(model_id,model_version) DO NOTHING",
    )
    .map_err(|e| e.to_string())?
    .execute(params![id, version])
    .map_err(|e| e.to_string())?;
    conn.prepare_cached(
        "SELECT model_pk FROM storage_clip_models WHERE model_id=?1 AND model_version=?2",
    )
    .map_err(|e| e.to_string())?
    .query_row(params![id, version], |r| r.get(0))
    .map_err(|e| e.to_string())
}

/// Restore old packages directly into their new destination, without an old DB copy.
pub(super) fn convert_staged(source: &Path, destination: &Path) -> Result<(), String> {
    let workspace = destination.parent().ok_or("目标数据库目录无效")?;
    let schema_root = workspace.join("compact-schema-contract");
    fs::create_dir(&schema_root).map_err(|e| e.to_string())?;
    fs::write(schema_root.join("compact_schema.sql"), SCHEMA).map_err(|e| e.to_string())?;
    fs::write(schema_root.join("compact_indexes.sql"), INDEXES).map_err(|e| e.to_string())?;
    let script = if let Some(root) = env::var_os("ILLUTAG_RESOURCE_ROOT") {
        PathBuf::from(root).join("scripts/storage_optimize.py")
    } else {
        let executable = env::current_exe().map_err(|e| e.to_string())?;
        let cwd = env::current_dir().map_err(|e| e.to_string())?;
        [
            executable
                .parent()
                .unwrap_or(Path::new("."))
                .join("scripts/storage_optimize.py"),
            cwd.join("scripts/storage_optimize.py"),
            cwd.join("src-tauri/scripts/storage_optimize.py"),
        ]
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("存储升级工具缺失")?
    };
    let output = python_command()
        .arg("-B")
        .arg(script)
        .arg("convert")
        .arg("--source")
        .arg(source)
        .arg("--target")
        .arg(destination)
        .arg("--workspace")
        .arg(workspace)
        .arg("--schema-root")
        .arg(&schema_root)
        .arg("--allow-other-drive")
        .output()
        .map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&schema_root);
    if !output.status.success() {
        return Err(format!(
            "旧图库转换失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
