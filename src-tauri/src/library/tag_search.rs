use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TagLabel {
    pub tag_en: String,
    pub tag_zh: Option<String>,
}

pub fn resolve_tag_labels(tags: Vec<String>, state: &AppState) -> Result<Vec<TagLabel>, String> {
    let conn = open_database(&state.database_path)?;
    let mut labels = Vec::new();
    let mut stmt = conn.prepare("SELECT COALESCE((SELECT NULLIF(tag_zh,'') FROM tag_dictionary WHERE tag_en=?1),(SELECT NULLIF(tag_zh,'') FROM known_image_tags WHERE tag_en=?1 LIMIT 1))").map_err(|e| e.to_string())?;
    for tag_en in tags.into_iter().take(200) {
        let tag_zh = stmt
            .query_row([&tag_en], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        labels.push(TagLabel { tag_en, tag_zh });
    }
    Ok(labels)
}

pub fn suggest_known_auto_tags(
    query: String,
    limit: Option<i64>,
    include_user_custom: Option<bool>,
    include_dictionary: Option<bool>,
    state: &AppState,
) -> Result<Vec<KnownAutoTagSuggestion>, String> {
    sync_tag_dictionary_from_source_if_changed(state)?;
    let keyword = query.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let conn = open_database(&state.database_path)?;
    let keyword_lower = keyword.to_lowercase();
    let like = format!("%{}%", escape_like_pattern(&keyword_lower));
    let like_prefix = format!("{}%", escape_like_pattern(&keyword_lower));
    let limit = limit.unwrap_or(20).clamp(1, 80);
    let include_user_custom = include_user_custom.unwrap_or(false);
    let include_dictionary = include_dictionary.unwrap_or(false);
    let mut stmt = conn
        .prepare(
            "
            WITH custom_tag_candidates AS (
              SELECT tag_text FROM user_custom_tags
              UNION
              SELECT tag_text FROM image_user_custom_tags
            ),
            custom_tag_counts AS (
              SELECT
                iuct.tag_text,
                COUNT(DISTINCT iuct.image_id) AS image_count
              FROM image_user_custom_tags iuct
              JOIN images i ON i.id = iuct.image_id
              WHERE i.source = 'library'
                AND COALESCE(i.trashed, 0) = 0
              GROUP BY iuct.tag_text
            ),
            dictionary_candidates AS (
              SELECT
                d.tag_en AS tag_en,
                d.tag_zh AS tag_zh
              FROM tag_dictionary d
              WHERE ?6 = 1
            ),
            candidates AS (
            SELECT
              k.tag_en AS tag_en,
              COALESCE(NULLIF(d.tag_zh, ''), NULLIF(k.tag_zh, ''), k.tag_en) AS tag_zh,
              k.image_count AS image_count,
              0 AS is_user_custom
            FROM known_image_tags k
            LEFT JOIN tag_dictionary d ON d.tag_en = k.tag_en
            WHERE k.model_name = ?1
              AND k.image_count > 0
            UNION ALL
            SELECT
              ct.tag_text AS tag_en,
              ct.tag_text AS tag_zh,
              COALESCE(cc.image_count, 0) AS image_count,
              1 AS is_user_custom
            FROM custom_tag_candidates ct
            LEFT JOIN custom_tag_counts cc ON cc.tag_text = ct.tag_text
            WHERE ?5 = 1
            UNION ALL
            SELECT
              dc.tag_en AS tag_en,
              dc.tag_zh AS tag_zh,
              0 AS image_count,
              0 AS is_user_custom
            FROM dictionary_candidates dc
            ),
            filtered AS (
              SELECT * FROM candidates
              WHERE LOWER(tag_en) LIKE ?2 ESCAPE '\\'
                 OR LOWER(COALESCE(tag_zh, '')) LIKE ?2 ESCAPE '\\'
            ),
            deduped AS (
              SELECT
                tag_en,
                MAX(COALESCE(tag_zh, '')) AS tag_zh,
                MAX(image_count) AS image_count,
                MAX(is_user_custom) AS is_user_custom
              FROM filtered
              GROUP BY tag_en
            )
            SELECT
              tag_en,
              NULLIF(tag_zh, '') AS tag_zh,
              image_count,
              is_user_custom
            FROM deduped
            ORDER BY
              CASE WHEN is_user_custom = 1 THEN 0 ELSE 1 END,
              CASE
                WHEN LOWER(COALESCE(tag_zh, '')) LIKE ?3 ESCAPE '\\' THEN 0
                WHEN LOWER(tag_en) LIKE ?3 ESCAPE '\\' THEN 1
                ELSE 2
              END,
              image_count DESC,
              tag_en COLLATE NOCASE
            LIMIT ?4
            ",
        )
        .map_err(|error| format!("Failed to prepare known tag suggestion query: {error}"))?;

    let rows = stmt
        .query_map(
            params![
                WD_TAGGER_MODEL_NAME,
                like,
                like_prefix,
                limit,
                if include_user_custom { 1 } else { 0 },
                if include_dictionary { 1 } else { 0 }
            ],
            |row| {
                Ok(KnownAutoTagSuggestion {
                    tag_en: row.get(0)?,
                    tag_zh: row.get(1)?,
                    image_count: row.get(2)?,
                    is_user_custom: row.get::<_, i64>(3)? != 0,
                })
            },
        )
        .map_err(|error| format!("Failed to query known tag suggestions: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to query known tag suggestions: {error}"))
}

pub(super) fn gallery_search_sql(filters: GallerySearchFilters) -> (String, Vec<Value>) {
    let (scope_where, mut params_values) = gallery_scope_where_sql(
        "images",
        filters.scope.as_deref().unwrap_or("all"),
        filters.folder_id,
        filters.unclassified_only_parent_folder_id,
    );
    let mut sql = format!(
        "
        SELECT images.id
        FROM images
        WHERE {}
        ",
        scope_where.as_ref()
    );

    let file_name_query = filters.file_name_query.trim().to_lowercase();
    if !file_name_query.is_empty() {
        sql.push_str(" AND LOWER(images.file_name) LIKE ? ESCAPE '\\'");
        params_values.push(Value::Text(format!(
            "%{}%",
            escape_like_pattern(&file_name_query)
        )));
    }

    let confidence_min = filters.confidence_min.clamp(0.0, 1.0);
    let confidence_max = filters.confidence_max.clamp(confidence_min, 1.0);
    let has_confidence_filter = confidence_min > 0.000_1 || confidence_max < 0.999_9;

    let mut zh_tags = filters
        .chinese_tag_ens
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    zh_tags.sort();
    zh_tags.dedup();
    let has_zh_tag_constraints = !zh_tags.is_empty();
    for tag_en in zh_tags {
        sql.push_str(
            "
            AND images.image_pk IN (
              SELECT t.image_pk
              FROM storage_auto_tags t
              WHERE t.model_pk = (SELECT string_pk FROM storage_strings WHERE value=?)
                AND t.tag_pk = (SELECT string_pk FROM storage_strings WHERE value=?)
                AND t.confidence BETWEEN ? AND ?
              UNION
              SELECT ust.image_id
              FROM storage_image_user_supplement_tags ust
              WHERE ust.tag_en = ?
              UNION
              SELECT uct.image_id
              FROM storage_image_user_custom_tags uct
              WHERE uct.tag_text = ?
            )
            ",
        );
        params_values.push(Value::Text(WD_TAGGER_MODEL_NAME.to_string()));
        params_values.push(Value::Text(tag_en.clone()));
        params_values.push(Value::Real(confidence_min as f64));
        params_values.push(Value::Real(confidence_max as f64));
        params_values.push(Value::Text(tag_en.clone()));
        params_values.push(Value::Text(tag_en));
    }

    let english_tokens = split_search_tokens(&filters.english_query);
    let has_english_constraints = !english_tokens.is_empty();
    for token in english_tokens {
        sql.push_str(
            "
            AND images.image_pk IN (
              SELECT t.image_pk
              FROM storage_auto_tags t
              WHERE t.model_pk = (SELECT string_pk FROM storage_strings WHERE value=?)
                AND t.tag_pk IN (
                  SELECT s.string_pk
                  FROM known_image_tags k JOIN storage_strings s ON s.value=k.tag_en
                  WHERE k.model_name = ?
                    AND LOWER(REPLACE(k.tag_en, '_', ' ')) LIKE ? ESCAPE '\\'
                )
                AND t.confidence BETWEEN ? AND ?
              UNION
              SELECT ust.image_id
              FROM storage_image_user_supplement_tags ust
              WHERE LOWER(REPLACE(ust.tag_en, '_', ' ')) LIKE ? ESCAPE '\\'
            )
            ",
        );
        params_values.push(Value::Text(WD_TAGGER_MODEL_NAME.to_string()));
        params_values.push(Value::Text(WD_TAGGER_MODEL_NAME.to_string()));
        let token_like = format!("%{}%", escape_like_pattern(&token));
        params_values.push(Value::Text(token_like.clone()));
        params_values.push(Value::Real(confidence_min as f64));
        params_values.push(Value::Real(confidence_max as f64));
        params_values.push(Value::Text(token_like));
    }

    if has_confidence_filter && !has_zh_tag_constraints && !has_english_constraints {
        sql.push_str(
            "
            AND EXISTS (
              SELECT 1
              FROM storage_auto_tags t
              WHERE t.image_pk = images.image_pk
                AND t.model_pk = (SELECT string_pk FROM storage_strings WHERE value=?)
                AND t.confidence BETWEEN ? AND ?
            )
            ",
        );
        params_values.push(Value::Text(WD_TAGGER_MODEL_NAME.to_string()));
        params_values.push(Value::Real(confidence_min as f64));
        params_values.push(Value::Real(confidence_max as f64));
    }

    sql.push_str(" ORDER BY images.modified_at DESC, images.path ASC");
    (sql, params_values)
}

pub fn search_gallery_image_ids(
    filters: GallerySearchFilters,
    state: &AppState,
) -> Result<Vec<String>, String> {
    let conn = open_database(&state.database_path)?;
    let (sql, params_values) = gallery_search_sql(filters);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("Failed to prepare gallery search query: {error}"))?;

    let rows = stmt
        .query_map(params_from_iter(params_values.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("Failed to run gallery search query: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to run gallery search query: {error}"))
}
