//! Tag bookmarks are small library preferences, included in existing backups.
use super::*;
const PREFIX: &str = "tag-favorite:v1:";

pub fn list_favorite_tags(state: &AppState) -> Result<Vec<String>, String> {
    let conn = open_database(&state.database_path)?;
    let mut stmt = conn
        .prepare("SELECT key FROM app_meta WHERE key GLOB 'tag-favorite:v1:*' ORDER BY key")
        .map_err(|e| e.to_string())?;
    let result = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|row| {
            row.map(|key| key[PREFIX.len()..].to_owned())
                .map_err(|e| e.to_string())
        })
        .collect();
    result
}

pub fn set_favorite_tag(
    tag: String,
    favorite: bool,
    state: &AppState,
) -> Result<Vec<String>, String> {
    if state.read_only || state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("图库只读或正在维护，暂时不能修改收藏标签".into());
    }
    let tag = tag.trim();
    if tag.is_empty() || tag.len() > 512 {
        return Err("标签无效".into());
    }
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let conn = open_database(&state.database_path)?;
    let key = format!("{PREFIX}{tag}");
    if favorite {
        conn.execute(
            "INSERT INTO app_meta(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO NOTHING",
            params![key, now_ms().to_string(), now_ms()],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute("DELETE FROM app_meta WHERE key=?1", [key])
            .map_err(|e| e.to_string())?;
    }
    list_favorite_tags(state)
}
