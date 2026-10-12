use super::*;

pub(super) fn normalize_user_folder_rule_logic(logic: &str) -> Option<&'static str> {
    match logic.trim().to_uppercase().as_str() {
        "AND" => Some("AND"),
        "OR" => Some("OR"),
        "NOT" => Some("NOT"),
        _ => None,
    }
}

pub(super) fn normalize_user_folder_rule_source(source: &str) -> Option<&'static str> {
    match source.trim().to_lowercase().as_str() {
        "danbooru" => Some("danbooru"),
        "custom" => Some("custom"),
        "filename" => Some("filename"),
        _ => None,
    }
}

pub(super) fn normalize_user_folder_rule_conditions(
    conditions: Vec<UserFolderRuleCondition>,
) -> Result<Vec<UserFolderRuleCondition>, String> {
    let mut normalized = Vec::<UserFolderRuleCondition>::with_capacity(conditions.len());
    for condition in conditions {
        let logic = normalize_user_folder_rule_logic(&condition.logic)
            .ok_or_else(|| format!("Unsupported rule logic: {}", condition.logic))?;
        let source = normalize_user_folder_rule_source(&condition.source)
            .ok_or_else(|| format!("Unsupported rule source: {}", condition.source))?;
        let keyword = condition.keyword.trim().to_string();
        if keyword.is_empty() {
            return Err("Rule keyword cannot be empty".to_string());
        }
        normalized.push(UserFolderRuleCondition {
            logic: logic.to_string(),
            source: source.to_string(),
            keyword,
        });
    }
    Ok(normalized)
}

pub(super) fn user_folder_exists(conn: &Connection, folder_id: i64) -> Result<bool, String> {
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM user_folders WHERE id = ?1)",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to query user folder: {error}"))?;
    Ok(exists != 0)
}

pub(super) fn user_folder_is_leaf(conn: &Connection, folder_id: i64) -> Result<bool, String> {
    let has_children: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM user_folders WHERE parent_id = ?1)",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to query user folder children: {error}"))?;
    Ok(has_children == 0)
}

pub(super) fn load_user_folder_rule(
    conn: &Connection,
    folder_id: i64,
) -> Result<Option<UserFolderRule>, String> {
    let row = conn
        .query_row(
            "
            SELECT rule_json, updated_at
            FROM user_folder_rules
            WHERE folder_id = ?1
            ",
            params![folder_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| format!("Failed to load user folder rule: {error}"))?;
    let Some((rule_json, updated_at)) = row else {
        return Ok(None);
    };
    let parsed: Vec<UserFolderRuleCondition> = serde_json::from_str(&rule_json)
        .map_err(|error| format!("Failed to parse stored user folder rule: {error}"))?;
    Ok(Some(UserFolderRule {
        folder_id,
        conditions: normalize_user_folder_rule_conditions(parsed)?,
        updated_at,
    }))
}

#[derive(Debug, Clone)]
pub(super) struct ImageFolderRuleMatchContext {
    file_name_lower: String,
    danbooru_terms: HashSet<String>,
    custom_terms: HashSet<String>,
}

pub(super) fn load_image_folder_rule_match_context(
    conn: &Connection,
    image_id: &str,
) -> Result<Option<ImageFolderRuleMatchContext>, String> {
    let image_row = conn
        .query_row(
            "
            SELECT file_name, source, COALESCE(trashed, 0)
            FROM images
            WHERE id = ?1
            ",
            params![image_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to load image info for folder rule: {error}"))?;
    let Some((file_name, source, trashed)) = image_row else {
        return Ok(None);
    };
    if source != "library" || trashed != 0 {
        return Ok(None);
    }

    let mut danbooru_terms = HashSet::<String>::new();
    {
        let mut stmt = conn
            .prepare(
                "
                SELECT tag_en, COALESCE(tag_zh, '')
                FROM image_auto_tags
                WHERE image_id = ?1
                ",
            )
            .map_err(|error| {
                format!("Failed to prepare auto tag query for folder rule: {error}")
            })?;
        let rows = stmt
            .query_map(params![image_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("Failed to query auto tags for folder rule: {error}"))?;
        for row in rows {
            let (tag_en, tag_zh) =
                row.map_err(|error| format!("Failed to query auto tags for folder rule: {error}"))?;
            let normalized_en = tag_en.trim().to_lowercase();
            if !normalized_en.is_empty() {
                danbooru_terms.insert(normalized_en);
            }
            let normalized_zh = tag_zh.trim().to_lowercase();
            if !normalized_zh.is_empty() {
                danbooru_terms.insert(normalized_zh);
            }
        }
    }
    {
        let mut stmt = conn
            .prepare(
                "
                SELECT tag_en, COALESCE(tag_zh, '')
                FROM image_user_supplement_tags
                WHERE image_id = ?1
                ",
            )
            .map_err(|error| {
                format!("Failed to prepare supplement tag query for folder rule: {error}")
            })?;
        let rows = stmt
            .query_map(params![image_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("Failed to query supplement tags for folder rule: {error}"))?;
        for row in rows {
            let (tag_en, tag_zh) = row.map_err(|error| {
                format!("Failed to query supplement tags for folder rule: {error}")
            })?;
            let normalized_en = tag_en.trim().to_lowercase();
            if !normalized_en.is_empty() {
                danbooru_terms.insert(normalized_en);
            }
            let normalized_zh = tag_zh.trim().to_lowercase();
            if !normalized_zh.is_empty() {
                danbooru_terms.insert(normalized_zh);
            }
        }
    }

    let mut custom_terms = HashSet::<String>::new();
    {
        let mut stmt = conn
            .prepare(
                "
                SELECT tag_text
                FROM image_user_custom_tags
                WHERE image_id = ?1
                ",
            )
            .map_err(|error| {
                format!("Failed to prepare custom tag query for folder rule: {error}")
            })?;
        let rows = stmt
            .query_map(params![image_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Failed to query custom tags for folder rule: {error}"))?;
        for row in rows {
            let tag_text = row
                .map_err(|error| format!("Failed to query custom tags for folder rule: {error}"))?;
            let normalized = tag_text.trim().to_lowercase();
            if !normalized.is_empty() {
                custom_terms.insert(normalized);
            }
        }
    }

    Ok(Some(ImageFolderRuleMatchContext {
        file_name_lower: file_name.to_lowercase(),
        danbooru_terms,
        custom_terms,
    }))
}

pub(super) fn evaluate_user_folder_rule_condition(
    context: &ImageFolderRuleMatchContext,
    condition: &UserFolderRuleCondition,
) -> bool {
    let keyword = condition.keyword.trim().to_lowercase();
    if keyword.is_empty() {
        return false;
    }
    match condition.source.as_str() {
        "danbooru" => context.danbooru_terms.contains(&keyword),
        "custom" => context.custom_terms.contains(&keyword),
        "filename" => context.file_name_lower.contains(&keyword),
        _ => false,
    }
}

pub(super) fn evaluate_user_folder_rule_conditions(
    context: &ImageFolderRuleMatchContext,
    conditions: &[UserFolderRuleCondition],
) -> bool {
    if conditions.is_empty() {
        return false;
    }
    let first = evaluate_user_folder_rule_condition(context, &conditions[0]);
    let mut result = if conditions[0].logic == "NOT" {
        !first
    } else {
        first
    };
    for condition in conditions.iter().skip(1) {
        let matched = evaluate_user_folder_rule_condition(context, condition);
        match condition.logic.as_str() {
            "AND" => result = result && matched,
            "OR" => result = result || matched,
            "NOT" => result = result && !matched,
            _ => {}
        }
    }
    result
}

pub(super) fn load_active_leaf_user_folder_rules(
    conn: &Connection,
) -> Result<Vec<(i64, Vec<UserFolderRuleCondition>)>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT r.folder_id, r.rule_json
            FROM user_folder_rules r
            JOIN user_folders f ON f.id = r.folder_id
            WHERE NOT EXISTS (
              SELECT 1
              FROM user_folders c
              WHERE c.parent_id = f.id
            )
            ",
        )
        .map_err(|error| format!("Failed to prepare user folder rules query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("Failed to query user folder rules: {error}"))?;
    let mut rules = Vec::<(i64, Vec<UserFolderRuleCondition>)>::new();
    for row in rows {
        let (folder_id, rule_json) =
            row.map_err(|error| format!("Failed to query user folder rules: {error}"))?;
        let parsed = match serde_json::from_str::<Vec<UserFolderRuleCondition>>(&rule_json) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[folder-rule] skip invalid rule json for folder {folder_id}: {error}");
                continue;
            }
        };
        let normalized = match normalize_user_folder_rule_conditions(parsed) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[folder-rule] skip invalid rule for folder {folder_id}: {error}");
                continue;
            }
        };
        if normalized.is_empty() {
            continue;
        }
        rules.push((folder_id, normalized));
    }
    Ok(rules)
}

pub(super) fn apply_matching_user_folder_rules_for_image(
    conn: &Connection,
    image_id: &str,
) -> Result<usize, String> {
    // 先查规则：没有任何规则时不必为每张图片加载匹配上下文
    let rules = load_active_leaf_user_folder_rules(conn)?;
    if rules.is_empty() {
        return Ok(0);
    }
    let Some(context) = load_image_folder_rule_match_context(conn, image_id)? else {
        return Ok(0);
    };
    let now = now_ms();
    let mut assigned = 0usize;
    for (folder_id, conditions) in rules {
        if !evaluate_user_folder_rule_conditions(&context, &conditions) {
            continue;
        }
        let affected = conn
            .execute(
                "
                INSERT OR IGNORE INTO image_user_folders (image_id, folder_id, assigned_at)
                VALUES (?1, ?2, ?3)
                ",
                params![image_id, folder_id, now],
            )
            .map_err(|error| format!("Failed to apply folder rule assignment: {error}"))?;
        assigned += affected;
    }
    Ok(assigned)
}

pub(super) fn apply_user_folder_rule_to_library_images(
    conn: &Connection,
    folder_id: i64,
    conditions: &[UserFolderRuleCondition],
) -> Result<usize, String> {
    if conditions.is_empty() {
        return Ok(0);
    }
    let mut stmt = conn
        .prepare(
            "
            SELECT id
            FROM images
            WHERE source = 'library'
              AND COALESCE(trashed, 0) = 0
            ",
        )
        .map_err(|error| format!("Failed to prepare folder rule apply candidates: {error}"))?;
    let image_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to query folder rule apply candidates: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to query folder rule apply candidates: {error}"))?;
    let now = now_ms();
    let mut assigned = 0usize;
    for image_id in image_ids {
        let Some(context) = load_image_folder_rule_match_context(conn, &image_id)? else {
            continue;
        };
        if !evaluate_user_folder_rule_conditions(&context, conditions) {
            continue;
        }
        let affected = conn
            .execute(
                "
                INSERT OR IGNORE INTO image_user_folders (image_id, folder_id, assigned_at)
                VALUES (?1, ?2, ?3)
                ",
                params![image_id, folder_id, now],
            )
            .map_err(|error| format!("Failed to apply folder rule assignment: {error}"))?;
        assigned += affected;
    }
    Ok(assigned)
}

pub fn get_user_folder_rule(
    folder_id: i64,
    state: &AppState,
) -> Result<Option<UserFolderRule>, String> {
    let conn = open_database(&state.database_path)?;
    if !user_folder_exists(&conn, folder_id)? {
        return Err("User folder not found".to_string());
    }
    load_user_folder_rule(&conn, folder_id)
}

pub fn save_user_folder_rule(
    folder_id: i64,
    conditions: Vec<UserFolderRuleCondition>,
    apply_now: bool,
    state: &AppState,
) -> Result<(), String> {
    let mut conn = open_database(&state.database_path)?;
    if !user_folder_exists(&conn, folder_id)? {
        return Err("User folder not found".to_string());
    }
    if !user_folder_is_leaf(&conn, folder_id)? {
        return Err("Only leaf folders can have rules".to_string());
    }

    let normalized = normalize_user_folder_rule_conditions(conditions)?;
    let now = now_ms();
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open folder rule transaction: {error}"))?;
    if normalized.is_empty() {
        tx.execute(
            "DELETE FROM user_folder_rules WHERE folder_id = ?1",
            params![folder_id],
        )
        .map_err(|error| format!("Failed to clear folder rule: {error}"))?;
    } else {
        let rule_json = serde_json::to_string(&normalized)
            .map_err(|error| format!("Failed to serialize folder rule: {error}"))?;
        tx.execute(
            "
            INSERT INTO user_folder_rules (folder_id, rule_json, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(folder_id) DO UPDATE SET
              rule_json = excluded.rule_json,
              updated_at = excluded.updated_at
            ",
            params![folder_id, rule_json, now],
        )
        .map_err(|error| format!("Failed to save folder rule: {error}"))?;
        if apply_now {
            apply_user_folder_rule_to_library_images(&tx, folder_id, &normalized)?;
        }
    }
    tx.commit()
        .map_err(|error| format!("Failed to commit folder rule transaction: {error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn create_user_folder(
    parent_id: Option<i64>,
    name: String,
    state: &AppState,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入文件夹名称".to_string());
    }

    let conn = open_database(&state.database_path)?;
    let now = now_ms();
    let sort_order = next_user_folder_sort_order(&conn, parent_id)?;

    conn.execute(
        "
        INSERT INTO user_folders (parent_id, name, sort_order, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?4)
        ",
        params![parent_id, name, sort_order, now],
    )
    .map_err(|error| format!("创建文件夹失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn rename_user_folder(folder_id: i64, name: String, state: &AppState) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入文件夹名称".to_string());
    }

    let conn = open_database(&state.database_path)?;
    conn.execute(
        "UPDATE user_folders SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now_ms(), folder_id],
    )
    .map_err(|error| format!("重命名文件夹失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn reorder_user_folder(
    folder_id: i64,
    target_folder_id: i64,
    state: &AppState,
) -> Result<(), String> {
    if folder_id == target_folder_id {
        return Ok(());
    }

    let mut conn = open_database(&state.database_path)?;

    let dragged_parent = user_folder_parent_id(&conn, folder_id)?;
    let target_parent = user_folder_parent_id(&conn, target_folder_id)?;
    if dragged_parent != target_parent {
        return Err("暂时只支持同层级文件夹排序".to_string());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("打开文件夹排序事务失败：{error}"))?;
    let sibling_ids = load_user_folder_sibling_ids(&tx, dragged_parent)?;
    let Some(from_index) = sibling_ids.iter().position(|id| *id == folder_id) else {
        return Err("找不到要排序的文件夹".to_string());
    };
    let Some(to_index) = sibling_ids.iter().position(|id| *id == target_folder_id) else {
        return Err("找不到目标文件夹".to_string());
    };

    let mut reordered = sibling_ids;
    let moved = reordered.remove(from_index);
    let insert_index = to_index;
    reordered.insert(insert_index, moved);

    for (index, id) in reordered.iter().enumerate() {
        tx.execute(
            "UPDATE user_folders SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![index as i64, now_ms(), id],
        )
        .map_err(|error| format!("保存文件夹排序失败：{error}"))?;
    }

    tx.commit()
        .map_err(|error| format!("保存文件夹排序失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn delete_user_folder(folder_id: i64, state: &AppState) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;

    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    conn.execute("DELETE FROM user_folders WHERE id = ?1", params![folder_id])
        .map_err(|error| format!("删除文件夹失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub(super) fn next_user_folder_sort_order(
    conn: &Connection,
    parent_id: Option<i64>,
) -> Result<i64, String> {
    conn.query_row(
        "
        SELECT COALESCE(MAX(sort_order), -1) + 1
        FROM user_folders
        WHERE parent_id IS ?1
        ",
        params![parent_id],
        |row| row.get(0),
    )
    .map_err(|error| format!("读取文件夹排序失败：{error}"))
}

pub(super) fn next_sort_order(
    conn: &Connection,
    table_name: &str,
    folder_id: Option<i64>,
) -> Result<i64, String> {
    let sql = match table_name {
        "reference_board_folders" => {
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM reference_board_folders"
        }
        "reference_boards" => {
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM reference_boards WHERE folder_id IS ?1"
        }
        _ => return Err("未知排序表".to_string()),
    };

    if table_name == "reference_boards" {
        conn.query_row(sql, params![folder_id], |row| row.get(0))
    } else {
        conn.query_row(sql, [], |row| row.get(0))
    }
    .map_err(|error| format!("读取排序失败：{error}"))
}

pub(super) fn user_folder_parent_id(
    conn: &Connection,
    folder_id: i64,
) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT parent_id FROM user_folders WHERE id = ?1",
        params![folder_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("读取文件夹层级失败：{error}"))?
    .ok_or_else(|| "找不到文件夹".to_string())
}

pub(super) fn load_user_folder_sibling_ids(
    conn: &Connection,
    parent_id: Option<i64>,
) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id
            FROM user_folders
            WHERE parent_id IS ?1
            ORDER BY sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取同级文件夹失败：{error}"))?;

    let ids = stmt
        .query_map(params![parent_id], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("读取同级文件夹失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取同级文件夹失败：{error}"))?;

    Ok(ids)
}

pub fn assign_image_to_user_folder(
    image_id: String,
    folder_id: i64,
    state: &AppState,
) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;

    let has_children: i64 = conn
        .query_row(
            "
            SELECT EXISTS(
              SELECT 1
              FROM user_folders
              WHERE parent_id = ?1
            )
            ",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查文件夹层级失败：{error}"))?;
    if has_children != 0 {
        return Err("只能将图片放入最小层级文件夹".to_string());
    }

    conn.execute(
        "
        INSERT OR IGNORE INTO image_user_folders (image_id, folder_id, assigned_at)
        VALUES (?1, ?2, ?3)
        ",
        params![image_id, folder_id, now_ms()],
    )
    .map_err(|error| format!("添加到文件夹失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn remove_image_from_user_folder(
    image_id: String,
    folder_id: i64,
    state: &AppState,
) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;

    conn.execute(
        "
        DELETE FROM image_user_folders
        WHERE image_id = ?1 AND folder_id = ?2
        ",
        params![image_id, folder_id],
    )
    .map_err(|error| format!("从文件夹移除图片失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn reapply_rules_batch(after: i64, state: &AppState) -> Result<(i64, i64, i64), String> {
    yield_to_foreground(
        &state.foreground_queries,
        &state.background_scan_stop_requested,
    );
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let total = count_gallery_images(&conn, Some(ACTIVE_GALLERY_IMAGE_WHERE))?;
    let ids=conn.prepare(&format!("SELECT i.rowid,i.id FROM images i WHERE i.rowid>?1 AND {ACTIVE_GALLERY_IMAGE_WHERE} ORDER BY i.rowid LIMIT 100")).map_err(|e|e.to_string())?.query_map([after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    let last = ids.last().map(|r| r.0).unwrap_or(after);
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for (_, id) in &ids {
        apply_matching_user_folder_rules_for_image(&tx, id)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok((last, ids.len() as i64, total))
}
