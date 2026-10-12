use super::*;

pub fn list_image_auto_tags(
    image_id: String,
    state: &AppState,
) -> Result<ImageAutoTagSummary, String> {
    sync_tag_dictionary_from_source_if_changed(state)?;
    let conn = open_database(&state.database_path)?;
    let mut stmt = conn
        .prepare(
            "
            SELECT
              t.category,
              t.tag_en,
              COALESCE(NULLIF(d.tag_zh, ''), NULLIF(t.tag_zh, ''), t.tag_en) AS tag_zh,
              t.confidence
            FROM image_auto_tags t
            LEFT JOIN tag_dictionary d ON d.tag_en = t.tag_en
            WHERE image_id = ?1
            ORDER BY
              CASE t.category WHEN 'character' THEN 0 WHEN 'general' THEN 1 ELSE 2 END,
              t.confidence DESC,
              t.tag_en COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to load image auto tags: {error}"))?;

    let tags = stmt
        .query_map(params![image_id.clone()], |row| {
            Ok(ImageAutoTag {
                category: row.get(0)?,
                tag_en: row.get(1)?,
                tag_zh: row.get(2)?,
                confidence: row.get(3)?,
            })
        })
        .map_err(|error| format!("Failed to load image auto tags: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load image auto tags: {error}"))?;

    let mut character_tags = Vec::<ImageAutoTag>::new();
    let mut general_tags = Vec::<ImageAutoTag>::new();
    for tag in tags {
        if tag.category == "character" {
            character_tags.push(tag);
        } else if tag.category == "general" {
            general_tags.push(tag);
        }
    }

    Ok(ImageAutoTagSummary {
        image_id,
        character_tags,
        general_tags,
    })
}

pub(super) fn normalize_tag_text(value: &str) -> String {
    value.trim().to_string()
}

pub(super) fn normalize_optional_tag_text(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

pub(super) fn upsert_user_custom_tag(conn: &Connection, tag_text: &str) -> Result<(), String> {
    let normalized = tag_text.trim();
    if normalized.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    conn.execute(
        "
        INSERT INTO user_custom_tags (tag_text, created_at, updated_at)
        VALUES (?1, ?2, ?2)
        ON CONFLICT(tag_text) DO UPDATE SET
          updated_at = excluded.updated_at
        ",
        params![normalized, now],
    )
    .map_err(|error| format!("Failed to upsert user custom tag: {error}"))?;
    Ok(())
}

pub(super) fn load_image_user_tag_summary(
    conn: &Connection,
    image_id: &str,
) -> Result<ImageUserTagSummary, String> {
    let mut custom_stmt = conn
        .prepare(
            "
            SELECT tag_text
            FROM image_user_custom_tags
            WHERE image_id = ?1
            ORDER BY updated_at DESC, tag_text COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to load custom user tags: {error}"))?;
    let custom_tags = custom_stmt
        .query_map(params![image_id], |row| {
            Ok(ImageUserCustomTag {
                tag_text: row.get(0)?,
            })
        })
        .map_err(|error| format!("Failed to load custom user tags: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load custom user tags: {error}"))?;

    let mut supplement_stmt = conn
        .prepare(
            "
            SELECT
              s.tag_en,
              COALESCE(NULLIF(d.tag_zh, ''), NULLIF(s.tag_zh, ''), s.tag_en) AS tag_zh
            FROM image_user_supplement_tags s
            LEFT JOIN tag_dictionary d ON d.tag_en = s.tag_en
            WHERE s.image_id = ?1
            ORDER BY s.updated_at DESC, s.tag_en COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to load supplement user tags: {error}"))?;
    let supplement_tags = supplement_stmt
        .query_map(params![image_id], |row| {
            Ok(ImageUserSupplementTag {
                tag_en: row.get(0)?,
                tag_zh: row.get(1)?,
            })
        })
        .map_err(|error| format!("Failed to load supplement user tags: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load supplement user tags: {error}"))?;

    Ok(ImageUserTagSummary {
        image_id: image_id.to_string(),
        custom_tags,
        supplement_tags,
    })
}

pub fn list_image_user_tags(
    image_id: String,
    state: &AppState,
) -> Result<ImageUserTagSummary, String> {
    sync_tag_dictionary_from_source_if_changed(state)?;
    let conn = open_database(&state.database_path)?;
    load_image_user_tag_summary(&conn, &image_id)
}

pub fn add_image_user_custom_tag(
    image_id: String,
    tag_text: String,
    state: &AppState,
) -> Result<ImageUserTagSummary, String> {
    let normalized = normalize_tag_text(&tag_text);
    if normalized.is_empty() {
        return Err("Tag text cannot be empty".to_string());
    }
    let mut conn = open_database(&state.database_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open custom user tag transaction: {error}"))?;
    let now = now_ms();
    tx.execute(
        "
        INSERT INTO image_user_custom_tags (
          image_id, tag_text, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?3)
        ON CONFLICT(image_id, tag_text) DO UPDATE SET
          updated_at = excluded.updated_at
        ",
        params![image_id, normalized, now],
    )
    .map_err(|error| format!("Failed to add custom user tag: {error}"))?;
    upsert_user_custom_tag(&tx, &normalized)?;
    apply_matching_user_folder_rules_for_image(&tx, &image_id)?;
    tx.commit()
        .map_err(|error| format!("Failed to commit custom user tag transaction: {error}"))?;
    invalidate_library_metadata(state)?;
    load_image_user_tag_summary(&conn, &image_id)
}

pub fn remove_image_user_custom_tag(
    image_id: String,
    tag_text: String,
    state: &AppState,
) -> Result<ImageUserTagSummary, String> {
    let normalized = normalize_tag_text(&tag_text);
    if normalized.is_empty() {
        return Err("Tag text cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "DELETE FROM image_user_custom_tags WHERE image_id = ?1 AND tag_text = ?2",
        params![image_id, normalized],
    )
    .map_err(|error| format!("Failed to remove custom user tag: {error}"))?;
    invalidate_library_metadata(state)?;
    load_image_user_tag_summary(&conn, &image_id)
}

pub fn add_image_user_supplement_tag(
    image_id: String,
    tag_en: String,
    tag_zh: Option<String>,
    state: &AppState,
) -> Result<ImageUserTagSummary, String> {
    let normalized_tag_en = normalize_tag_text(&tag_en);
    if normalized_tag_en.is_empty() {
        return Err("Tag text cannot be empty".to_string());
    }
    let normalized_tag_zh = normalize_optional_tag_text(tag_zh);
    let mut conn = open_database(&state.database_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open supplement user tag transaction: {error}"))?;
    let now = now_ms();
    tx.execute(
        "
        INSERT INTO image_user_supplement_tags (
          image_id, tag_en, tag_zh, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        ON CONFLICT(image_id, tag_en) DO UPDATE SET
          tag_zh = COALESCE(excluded.tag_zh, image_user_supplement_tags.tag_zh),
          updated_at = excluded.updated_at
        ",
        params![image_id, normalized_tag_en, normalized_tag_zh, now],
    )
    .map_err(|error| format!("Failed to add supplement user tag: {error}"))?;
    apply_matching_user_folder_rules_for_image(&tx, &image_id)?;
    tx.commit()
        .map_err(|error| format!("Failed to commit supplement user tag transaction: {error}"))?;
    invalidate_library_metadata(state)?;
    load_image_user_tag_summary(&conn, &image_id)
}

pub fn remove_image_user_supplement_tag(
    image_id: String,
    tag_en: String,
    state: &AppState,
) -> Result<ImageUserTagSummary, String> {
    let normalized_tag_en = normalize_tag_text(&tag_en);
    if normalized_tag_en.is_empty() {
        return Err("Tag text cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "DELETE FROM image_user_supplement_tags WHERE image_id = ?1 AND tag_en = ?2",
        params![image_id, normalized_tag_en],
    )
    .map_err(|error| format!("Failed to remove supplement user tag: {error}"))?;
    invalidate_library_metadata(state)?;
    load_image_user_tag_summary(&conn, &image_id)
}

pub(super) fn cleanup_orphan_user_tag_folder_members(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "
        DELETE FROM user_tag_folder_members
        WHERE tag_text NOT IN (
          SELECT tag_text FROM user_custom_tags
          UNION
          SELECT DISTINCT tag_text FROM image_user_custom_tags
        )
        ",
        [],
    )
    .map_err(|error| format!("Failed to cleanup orphan user tag members: {error}"))?;
    Ok(())
}

pub(super) fn load_tag_management_state(conn: &Connection) -> Result<TagManagementState, String> {
    cleanup_orphan_user_tag_folder_members(conn)?;

    let mut folder_stmt = conn
        .prepare(
            "
            SELECT id, name, sort_order
            FROM user_tag_folders
            ORDER BY sort_order ASC, id ASC
            ",
        )
        .map_err(|error| format!("Failed to prepare user tag folders query: {error}"))?;

    let folder_rows = folder_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("Failed to query user tag folders: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to query user tag folders: {error}"))?;

    let mut tag_stmt = conn
        .prepare(
            "
            SELECT m.tag_text
            FROM user_tag_folder_members m
            JOIN (
              SELECT tag_text FROM user_custom_tags
              UNION
              SELECT DISTINCT tag_text FROM image_user_custom_tags
            ) c ON c.tag_text = m.tag_text
            WHERE m.folder_id = ?1
            ORDER BY m.tag_text COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to prepare user tag folder tags query: {error}"))?;

    let mut folders = Vec::<UserTagFolder>::with_capacity(folder_rows.len());
    for (id, name, sort_order) in folder_rows {
        let tags = tag_stmt
            .query_map(params![id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Failed to query user tag folder tags: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to query user tag folder tags: {error}"))?;
        folders.push(UserTagFolder {
            id,
            name,
            sort_order,
            tags,
        });
    }

    let mut unclassified_stmt = conn
        .prepare(
            "
            SELECT c.tag_text
            FROM (
              SELECT tag_text FROM user_custom_tags
              UNION
              SELECT DISTINCT tag_text FROM image_user_custom_tags
            ) c
            LEFT JOIN user_tag_folder_members m ON m.tag_text = c.tag_text
            WHERE m.tag_text IS NULL
            ORDER BY c.tag_text COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to prepare unclassified user tags query: {error}"))?;
    let unclassified_tags = unclassified_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to query unclassified user tags: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to query unclassified user tags: {error}"))?;

    Ok(TagManagementState {
        folders,
        unclassified_tags,
    })
}

pub fn list_tag_management_state(state: &AppState) -> Result<TagManagementState, String> {
    let conn = open_database(&state.database_path)?;
    load_tag_management_state(&conn)
}

pub fn list_tag_dictionary_browser(state: &AppState) -> Result<TagDictionaryBrowserState, String> {
    sync_tag_dictionary_from_source_if_changed(state)?;
    let conn = open_database(&state.database_path)?;

    let mut zh_map = HashMap::<String, String>::new();
    {
        let mut stmt = conn
            .prepare("SELECT tag_en, tag_zh FROM tag_dictionary")
            .map_err(|error| format!("Failed to prepare tag dictionary query: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("Failed to query tag dictionary: {error}"))?;
        for row in rows {
            let (tag_en, tag_zh) =
                row.map_err(|error| format!("Failed to read tag dictionary: {error}"))?;
            if !tag_zh.is_empty() {
                zh_map.insert(tag_en, tag_zh);
            }
        }
    }

    let mut count_map = HashMap::<String, i64>::new();
    {
        let mut stmt = conn
            .prepare(
                "
                SELECT tag_en, image_count
                FROM known_image_tags
                WHERE model_name = ?1
                ",
            )
            .map_err(|error| format!("Failed to prepare library tag counts query: {error}"))?;
        let rows = stmt
            .query_map(params![WD_TAGGER_MODEL_NAME], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| format!("Failed to query library tag counts: {error}"))?;
        for row in rows {
            let (tag_en, image_count) =
                row.map_err(|error| format!("Failed to read library tag counts: {error}"))?;
            count_map.insert(tag_en, image_count);
        }
    }

    let (groups, group_by_tag, sort_index) = load_tag_dictionary_browser_meta()?;
    let mut tags = Vec::<TagDictionaryBrowserItem>::with_capacity(group_by_tag.len());
    for (tag_en, group) in group_by_tag {
        tags.push(TagDictionaryBrowserItem {
            tag_zh: zh_map.get(&tag_en).cloned(),
            image_count: count_map.get(&tag_en).copied().unwrap_or(0),
            sort_index: sort_index.get(&tag_en).copied().unwrap_or(i64::MAX),
            tag_en,
            group,
        });
    }

    Ok(TagDictionaryBrowserState { groups, tags })
}

#[derive(Debug, Deserialize)]
struct TagGroupsFile {
    #[serde(default)]
    taxonomy: Vec<TagDictionaryGroup>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

pub(super) fn tagger_data_dir_candidates() -> Result<Vec<PathBuf>, String> {
    if let Some(path) = explicit_resource_path("wd-swinv2-tagger-v3")? {
        return path
            .is_dir()
            .then_some(vec![path.clone()])
            .ok_or_else(|| format!("隔离标签资源缺失：{}", path.display()));
    }
    let mut candidates = resource_candidates(
        &["wd-swinv2-tagger-v3"],
        &["wd-swinv2-tagger-v3", "../wd-swinv2-tagger-v3"],
        &["../wd-swinv2-tagger-v3", "../../wd-swinv2-tagger-v3"],
    );
    if let Ok(dictionary_path) = resolve_dictionary_source_path() {
        if let Some(parent) = dictionary_path.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    Ok(candidates)
}

pub(super) fn find_tagger_data_file(file_name: &str) -> Result<Option<PathBuf>, String> {
    for dir in tagger_data_dir_candidates()? {
        let path = dir.join(file_name);
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

pub(super) fn load_selected_tag_sort_index(path: &Path) -> Result<HashMap<String, i64>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let mut sort_index = HashMap::<String, i64>::new();
    let mut index: i64 = 0;
    for (line_index, line) in content.lines().enumerate() {
        let fields = parse_csv_line(line.trim());
        let name = fields.get(1).map(|value| value.trim()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        if line_index == 0 && name.eq_ignore_ascii_case("name") {
            continue;
        }
        sort_index.entry(name.to_string()).or_insert(index);
        index += 1;
    }
    Ok(sort_index)
}

pub(super) fn load_tag_groups_file(
    path: &Path,
) -> Result<(Vec<TagDictionaryGroup>, HashMap<String, String>), String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let parsed: TagGroupsFile = serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse tag_groups.json: {error}"))?;
    let mut groups = parsed.taxonomy;
    let mut seen = groups
        .iter()
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();
    for group in parsed.tags.values() {
        if seen.insert(group.clone()) {
            groups.push(TagDictionaryGroup {
                id: group.clone(),
                zh: group.clone(),
            });
        }
    }
    Ok((groups, parsed.tags))
}

pub(super) fn load_fallback_groups_from_selected_tags(
    path: &Path,
) -> Result<(Vec<TagDictionaryGroup>, HashMap<String, String>), String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let mut group_by_tag = HashMap::<String, String>::new();
    for (line_index, line) in content.lines().enumerate() {
        let fields = parse_csv_line(line.trim());
        let name = fields.get(1).map(|value| value.trim()).unwrap_or("");
        let category = fields.get(2).map(|value| value.trim()).unwrap_or("0");
        if name.is_empty() {
            continue;
        }
        if line_index == 0 && name.eq_ignore_ascii_case("name") {
            continue;
        }
        let group = if category == "4" {
            "character"
        } else {
            "general"
        };
        group_by_tag.insert(name.to_string(), group.to_string());
    }
    Ok((
        vec![
            TagDictionaryGroup {
                id: "character".to_string(),
                zh: "角色".to_string(),
            },
            TagDictionaryGroup {
                id: "general".to_string(),
                zh: "通用".to_string(),
            },
        ],
        group_by_tag,
    ))
}

pub(super) fn load_tag_dictionary_browser_meta() -> Result<
    (
        Vec<TagDictionaryGroup>,
        HashMap<String, String>,
        HashMap<String, i64>,
    ),
    String,
> {
    let sort_index = match find_tagger_data_file("selected_tags.csv")? {
        Some(path) => load_selected_tag_sort_index(&path)?,
        None => HashMap::new(),
    };
    if let Some(path) = find_tagger_data_file("tag_groups.json")? {
        let (groups, group_by_tag) = load_tag_groups_file(&path)?;
        return Ok((groups, group_by_tag, sort_index));
    }
    if let Some(path) = find_tagger_data_file("selected_tags.csv")? {
        let (groups, group_by_tag) = load_fallback_groups_from_selected_tags(&path)?;
        return Ok((groups, group_by_tag, sort_index));
    }
    Ok((Vec::new(), HashMap::new(), sort_index))
}
