use super::*;

pub(super) fn open_database(database_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建图库目录失败：{error}"))?;
    }

    let readonly = env::var("ILLUTAG_READ_ONLY").as_deref() == Ok("1");
    let conn = if readonly {
        Connection::open_with_flags(database_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    } else {
        Connection::open(database_path)
    }
    .map_err(|error| format!("打开图库数据库失败：{error}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(|error| format!("Failed to set SQLite busy_timeout: {error}"))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| format!("Failed to enable foreign keys: {error}"))?;
    conn.execute_batch(
        "
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -262144;
        PRAGMA mmap_size = 8589934592;
        ",
    )
    .map_err(|error| format!("Failed to tune SQLite pragmas: {error}"))?;
    let schema_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("读取数据库版本失败：{e}"))?;
    if schema_version > SCHEMA_USER_VERSION {
        return Err(format!("数据库版本 {schema_version} 高于此程序支持的版本 {SCHEMA_USER_VERSION}，请使用匹配的新版本"));
    }
    if schema_version == 0 && !readonly {
        compact::initialize(&conn)?;
    } else if schema_version < SCHEMA_USER_VERSION {
        return Err("此图库需要离线存储升级，请先运行存储优化工具；原数据库未修改".into());
    }
    if !readonly && schema_version == SCHEMA_USER_VERSION {
        // Auxiliary table added without changing the schema version. Existing
        // schema-3 libraries get it on the first writable open; readonly
        // diagnostics must not attempt DDL.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scan_pending_assignments (
               image_pk INTEGER PRIMARY KEY,
               created_at INTEGER NOT NULL,
               FOREIGN KEY(image_pk) REFERENCES images(image_pk) ON DELETE CASCADE
             );",
        )
        .map_err(|error| format!("Failed to ensure scan pending table: {error}"))?;
    }
    Ok(conn)
}

#[cfg(test)]
pub(super) fn migrate_database(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS folders (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          added_at INTEGER NOT NULL,
          last_scanned_at INTEGER,
          hidden INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS images (
          id TEXT PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          file_name TEXT NOT NULL,
          ext TEXT NOT NULL,
          width INTEGER NOT NULL,
          height INTEGER NOT NULL,
          file_size INTEGER NOT NULL,
          modified_at INTEGER NOT NULL,
          imported_at INTEGER NOT NULL,
          folder_id INTEGER NOT NULL,
          missing INTEGER NOT NULL DEFAULT 0,
          trashed INTEGER NOT NULL DEFAULT 0,
          is_favorite INTEGER NOT NULL DEFAULT 0,
          content_hash TEXT,
          source TEXT NOT NULL DEFAULT 'library',
          FOREIGN KEY(folder_id) REFERENCES folders(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_images_folder_id ON images(folder_id);
        CREATE INDEX IF NOT EXISTS idx_images_modified_at ON images(modified_at DESC);

        CREATE TABLE IF NOT EXISTS tags (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          namespace TEXT,
          source TEXT NOT NULL DEFAULT 'manual',
          UNIQUE(name, namespace, source)
        );

        CREATE TABLE IF NOT EXISTS image_tags (
          image_id TEXT NOT NULL,
          tag_id INTEGER NOT NULL,
          confidence REAL,
          source TEXT NOT NULL DEFAULT 'manual',
          PRIMARY KEY(image_id, tag_id, source),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE,
          FOREIGN KEY(tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS user_folders (
          id INTEGER PRIMARY KEY,
          parent_id INTEGER,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          source_kind TEXT NOT NULL DEFAULT 'manual',
          source_path TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(parent_id) REFERENCES user_folders(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_user_folders_parent_id ON user_folders(parent_id);

        CREATE TABLE IF NOT EXISTS image_user_folders (
          image_id TEXT NOT NULL,
          folder_id INTEGER NOT NULL,
          assigned_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, folder_id),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE,
          FOREIGN KEY(folder_id) REFERENCES user_folders(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_user_folders_folder_id ON image_user_folders(folder_id);

        CREATE TABLE IF NOT EXISTS user_folder_rules (
          folder_id INTEGER PRIMARY KEY,
          rule_json TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(folder_id) REFERENCES user_folders(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS ai_models (
          id INTEGER PRIMARY KEY,
          kind TEXT NOT NULL,
          name TEXT NOT NULL,
          version TEXT,
          metadata_json TEXT,
          UNIQUE(kind, name, version)
        );

        CREATE TABLE IF NOT EXISTS image_ai_state (
          image_id TEXT NOT NULL,
          model_id INTEGER NOT NULL,
          status TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, model_id),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE,
          FOREIGN KEY(model_id) REFERENCES ai_models(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS tag_dictionary (
          tag_en TEXT PRIMARY KEY,
          tag_zh TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS image_auto_tags (
          image_id TEXT NOT NULL,
          category TEXT NOT NULL,
          tag_en TEXT NOT NULL,
          tag_zh TEXT,
          confidence REAL NOT NULL,
          model_name TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, category, tag_en, model_name),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_auto_tags_image_id
          ON image_auto_tags(image_id, category, confidence DESC);

        CREATE INDEX IF NOT EXISTS idx_image_auto_tags_tag_image
          ON image_auto_tags(model_name, tag_en, image_id, confidence);

        CREATE TABLE IF NOT EXISTS known_image_tags (
          model_name TEXT NOT NULL,
          tag_en TEXT NOT NULL,
          tag_zh TEXT,
          image_count INTEGER NOT NULL DEFAULT 0,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(model_name, tag_en)
        );

        CREATE INDEX IF NOT EXISTS idx_known_image_tags_model_zh
          ON known_image_tags(model_name, tag_zh);
        CREATE INDEX IF NOT EXISTS idx_known_image_tags_model_en
          ON known_image_tags(model_name, tag_en);

        CREATE TABLE IF NOT EXISTS app_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS image_user_custom_tags (
          image_id TEXT NOT NULL,
          tag_text TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, tag_text),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_user_custom_tags_image_id
          ON image_user_custom_tags(image_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS image_user_supplement_tags (
          image_id TEXT NOT NULL,
          tag_en TEXT NOT NULL,
          tag_zh TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, tag_en),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_user_supplement_tags_image_id
          ON image_user_supplement_tags(image_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS user_custom_tags (
          tag_text TEXT PRIMARY KEY,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_tag_folders (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_tag_folder_members (
          folder_id INTEGER NOT NULL,
          tag_text TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(folder_id, tag_text),
          UNIQUE(tag_text),
          FOREIGN KEY(folder_id) REFERENCES user_tag_folders(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_user_tag_folder_members_folder_id
          ON user_tag_folder_members(folder_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS image_thumbnails (
          image_id TEXT PRIMARY KEY,
          thumb_path TEXT NOT NULL,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_thumbnails_updated_at
          ON image_thumbnails(updated_at DESC);

        CREATE TABLE IF NOT EXISTS image_clip_embeddings (
          image_id TEXT NOT NULL,
          model_id TEXT NOT NULL,
          model_version TEXT NOT NULL,
          dimension INTEGER NOT NULL,
          normalized INTEGER NOT NULL DEFAULT 1,
          vector_blob BLOB NOT NULL,
          source_modified_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, model_id, model_version),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_clip_embeddings_model_updated
          ON image_clip_embeddings(model_id, model_version, updated_at DESC);

        CREATE TABLE IF NOT EXISTS image_atmosphere_signatures (
          image_id TEXT PRIMARY KEY,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          signature_blob BLOB NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_atmosphere_signatures_updated
          ON image_atmosphere_signatures(updated_at DESC);

        CREATE TABLE IF NOT EXISTS image_color_signatures (
          image_id TEXT PRIMARY KEY,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          signature_blob BLOB NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_color_signatures_updated
          ON image_color_signatures(updated_at DESC);

        CREATE TRIGGER IF NOT EXISTS trg_image_auto_tags_insert_known
        AFTER INSERT ON image_auto_tags
        WHEN NOT EXISTS (
          SELECT 1
          FROM image_auto_tags t
          WHERE t.image_id = NEW.image_id
            AND t.model_name = NEW.model_name
            AND t.tag_en = NEW.tag_en
            AND t.rowid <> NEW.rowid
        )
        BEGIN
          INSERT INTO known_image_tags (model_name, tag_en, tag_zh, image_count, updated_at)
          VALUES (NEW.model_name, NEW.tag_en, NEW.tag_zh, 1, NEW.updated_at)
          ON CONFLICT(model_name, tag_en) DO UPDATE SET
            image_count = known_image_tags.image_count + 1,
            tag_zh = COALESCE(excluded.tag_zh, known_image_tags.tag_zh),
            updated_at = excluded.updated_at;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_image_auto_tags_update_known
        AFTER UPDATE OF tag_zh, updated_at ON image_auto_tags
        BEGIN
          UPDATE known_image_tags
          SET
            tag_zh = COALESCE(NEW.tag_zh, known_image_tags.tag_zh),
            updated_at = NEW.updated_at
          WHERE model_name = NEW.model_name
            AND tag_en = NEW.tag_en;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_image_auto_tags_delete_known
        AFTER DELETE ON image_auto_tags
        WHEN NOT EXISTS (
          SELECT 1
          FROM image_auto_tags t
          WHERE t.image_id = OLD.image_id
            AND t.model_name = OLD.model_name
            AND t.tag_en = OLD.tag_en
        )
        BEGIN
          UPDATE known_image_tags
          SET
            image_count = MAX(0, image_count - 1),
            updated_at = OLD.updated_at
          WHERE model_name = OLD.model_name
            AND tag_en = OLD.tag_en;
          DELETE FROM known_image_tags
          WHERE model_name = OLD.model_name
            AND tag_en = OLD.tag_en
            AND image_count <= 0;
        END;

        CREATE TABLE IF NOT EXISTS reference_board_folders (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS reference_boards (
          id INTEGER PRIMARY KEY,
          folder_id INTEGER,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(folder_id) REFERENCES reference_board_folders(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_reference_boards_folder_id
          ON reference_boards(folder_id);

        CREATE TABLE IF NOT EXISTS reference_board_items (
          id INTEGER PRIMARY KEY,
          board_id INTEGER NOT NULL,
          image_id TEXT NOT NULL,
          x REAL NOT NULL DEFAULT 0,
          y REAL NOT NULL DEFAULT 0,
          width REAL NOT NULL DEFAULT 220,
          height REAL NOT NULL DEFAULT 220,
          rotation REAL NOT NULL DEFAULT 0,
          flip_x INTEGER NOT NULL DEFAULT 0,
          flip_y INTEGER NOT NULL DEFAULT 0,
          z_index INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          FOREIGN KEY(board_id) REFERENCES reference_boards(id) ON DELETE CASCADE,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_reference_board_items_board_id
          ON reference_board_items(board_id, z_index);
        ",
    )
    .map_err(|error| format!("初始化图库数据库失败：{error}"))?;

    ensure_library_columns(conn)?;
    ensure_thumbnail_table(conn)?;
    ensure_clip_embedding_table(conn)?;
    ensure_user_folder_sort_order(conn)?;
    ensure_user_folder_source_metadata(conn)?;
    ensure_reference_board_items_allow_duplicates(conn)?;
    ensure_reference_board_item_flip_columns(conn)?;
    ensure_known_image_tags_bootstrap(conn)?;

    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_known_image_tags_bootstrap(conn: &Connection) -> Result<(), String> {
    let known_count = conn
        .query_row("SELECT COUNT(*) FROM known_image_tags", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| format!("Failed to read known_image_tags count: {error}"))?;
    if known_count > 0 {
        return Ok(());
    }
    let auto_tag_count = conn
        .query_row("SELECT COUNT(*) FROM image_auto_tags", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| format!("Failed to read image_auto_tags count: {error}"))?;
    if auto_tag_count == 0 {
        return Ok(());
    }
    rebuild_known_image_tags(conn)
}

#[cfg(test)]
pub(super) fn rebuild_known_image_tags(conn: &Connection) -> Result<(), String> {
    let now = now_ms();
    conn.execute("DELETE FROM known_image_tags", [])
        .map_err(|error| format!("Failed to reset known image tags: {error}"))?;
    conn.execute(
        "
        INSERT INTO known_image_tags (model_name, tag_en, tag_zh, image_count, updated_at)
        SELECT
          image_auto_tags.model_name,
          image_auto_tags.tag_en,
          MAX(NULLIF(image_auto_tags.tag_zh, '')) AS tag_zh,
          COUNT(DISTINCT image_auto_tags.image_id) AS image_count,
          ?1
        FROM image_auto_tags
        JOIN images ON images.id = image_auto_tags.image_id
        WHERE images.source = 'library'
          AND COALESCE(images.trashed, 0) = 0
        GROUP BY image_auto_tags.model_name, image_auto_tags.tag_en
        ",
        params![now],
    )
    .map_err(|error| format!("Failed to rebuild known image tags: {error}"))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_library_columns(conn: &Connection) -> Result<(), String> {
    if !table_has_column(conn, "folders", "hidden")? {
        conn.execute(
            "ALTER TABLE folders ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("升级图库文件夹隐藏字段失败：{error}"))?;
    }
    if !table_has_column(conn, "images", "source")? {
        conn.execute(
            "ALTER TABLE images ADD COLUMN source TEXT NOT NULL DEFAULT 'library'",
            [],
        )
        .map_err(|error| format!("升级图片来源字段失败：{error}"))?;
    }
    if !table_has_column(conn, "images", "trashed")? {
        conn.execute(
            "ALTER TABLE images ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("Failed to upgrade images.trashed column: {error}"))?;
    }
    if !table_has_column(conn, "images", "is_favorite")? {
        conn.execute(
            "ALTER TABLE images ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("Failed to upgrade images.is_favorite column: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_thumbnail_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS image_thumbnails (
          image_id TEXT PRIMARY KEY,
          thumb_path TEXT NOT NULL,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_image_thumbnails_updated_at
          ON image_thumbnails(updated_at DESC);
        ",
    )
    .map_err(|error| format!("Failed to ensure image_thumbnails table: {error}"))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_clip_embedding_table(conn: &Connection) -> Result<(), String> {
    if !table_exists(conn, "image_clip_embeddings")? {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS image_clip_embeddings (
              image_id TEXT NOT NULL,
              model_id TEXT NOT NULL,
              model_version TEXT NOT NULL,
              dimension INTEGER NOT NULL,
              normalized INTEGER NOT NULL DEFAULT 1,
              vector_blob BLOB NOT NULL,
              source_modified_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL,
              PRIMARY KEY(image_id, model_id, model_version),
              FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_image_clip_embeddings_model_updated
              ON image_clip_embeddings(model_id, model_version, updated_at DESC);
            ",
        )
        .map_err(|error| format!("Failed to ensure image_clip_embeddings table: {error}"))?;
        return Ok(());
    }

    if table_has_column(conn, "image_clip_embeddings", "vector_blob")?
        && table_has_column(conn, "image_clip_embeddings", "model_id")?
        && table_has_column(conn, "image_clip_embeddings", "model_version")?
        && table_has_column(conn, "image_clip_embeddings", "dimension")?
        && table_has_column(conn, "image_clip_embeddings", "normalized")?
    {
        return Ok(());
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS image_clip_embeddings_v2 (
          image_id TEXT NOT NULL,
          model_id TEXT NOT NULL,
          model_version TEXT NOT NULL,
          dimension INTEGER NOT NULL,
          normalized INTEGER NOT NULL DEFAULT 1,
          vector_blob BLOB NOT NULL,
          source_modified_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, model_id, model_version),
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );
        ",
    )
    .map_err(|error| format!("Failed to create image_clip_embeddings_v2: {error}"))?;

    let has_vector_json = table_has_column(conn, "image_clip_embeddings", "vector_json")?;
    if has_vector_json {
        let mut stmt = conn
            .prepare(
                "
                SELECT image_id, model_name, dim, vector_json, source_modified_at, updated_at
                FROM image_clip_embeddings
                ",
            )
            .map_err(|error| format!("Failed to read legacy clip embeddings: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|error| format!("Failed to read legacy clip embeddings: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read legacy clip embeddings: {error}"))?;
        drop(stmt);

        for (
            image_id,
            legacy_model_name,
            legacy_dim,
            vector_json,
            source_modified_at,
            updated_at,
        ) in rows
        {
            let parsed: Vec<f32> = match serde_json::from_str(&vector_json) {
                Ok(values) => values,
                Err(_) => continue,
            };
            if parsed.is_empty() {
                continue;
            }
            let normalized = normalize_vector(&parsed);
            let vector_blob = encode_f32_blob(&normalized);
            let dimension = legacy_dim
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .unwrap_or(normalized.len());
            let model_id = legacy_model_name
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(legacy_model_name_to_model_id)
                .unwrap_or(CHINESE_CLIP_MODEL_ID)
                .to_string();

            conn.execute(
                "
                INSERT OR REPLACE INTO image_clip_embeddings_v2 (
                  image_id, model_id, model_version, dimension, normalized, vector_blob, source_modified_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)
                ",
                params![
                    image_id,
                    model_id,
                    CHINESE_CLIP_MODEL_VERSION,
                    i64::try_from(dimension).unwrap_or(0),
                    vector_blob,
                    source_modified_at,
                    updated_at,
                ],
            )
            .map_err(|error| format!("Failed to migrate legacy clip embedding: {error}"))?;
        }
    }

    conn.execute_batch(
        "
        DROP TABLE IF EXISTS image_clip_embeddings;
        ALTER TABLE image_clip_embeddings_v2 RENAME TO image_clip_embeddings;
        CREATE INDEX IF NOT EXISTS idx_image_clip_embeddings_model_updated
          ON image_clip_embeddings(model_id, model_version, updated_at DESC);
        ",
    )
    .map_err(|error| format!("Failed to finalize clip embedding migration: {error}"))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        params![table_name],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|error| format!("Failed to inspect table existence: {error}"))
}

#[cfg(test)]
pub(super) fn table_has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .map_err(|error| format!("读取表结构失败：{error}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("读取表结构失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取表结构失败：{error}"))?;
    Ok(columns.iter().any(|column| column == column_name))
}

#[cfg(test)]
pub(super) fn ensure_reference_board_items_allow_duplicates(
    conn: &Connection,
) -> Result<(), String> {
    let has_unique_constraint = conn
        .prepare("PRAGMA index_list(reference_board_items)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| format!("读取参考板图片索引失败：{error}"))?
        .iter()
        .any(|(_, is_unique)| *is_unique);

    if !has_unique_constraint {
        return Ok(());
    }

    conn.execute_batch(
        "
        ALTER TABLE reference_board_items RENAME TO reference_board_items_old;
        CREATE TABLE reference_board_items (
          id INTEGER PRIMARY KEY,
          board_id INTEGER NOT NULL,
          image_id TEXT NOT NULL,
          x REAL NOT NULL DEFAULT 0,
          y REAL NOT NULL DEFAULT 0,
          width REAL NOT NULL DEFAULT 220,
          height REAL NOT NULL DEFAULT 220,
          rotation REAL NOT NULL DEFAULT 0,
          flip_x INTEGER NOT NULL DEFAULT 0,
          flip_y INTEGER NOT NULL DEFAULT 0,
          z_index INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          FOREIGN KEY(board_id) REFERENCES reference_boards(id) ON DELETE CASCADE,
          FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
        );
        INSERT INTO reference_board_items (
          id, board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        )
        SELECT id, board_id, image_id, x, y, width, height, rotation, 0, 0, z_index, created_at
        FROM reference_board_items_old;
        DROP TABLE reference_board_items_old;
        CREATE INDEX IF NOT EXISTS idx_reference_board_items_board_id
          ON reference_board_items(board_id, z_index);
        ",
    )
    .map_err(|error| format!("升级参考板图片表失败：{error}"))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_reference_board_item_flip_columns(conn: &Connection) -> Result<(), String> {
    if !table_has_column(conn, "reference_board_items", "flip_x")? {
        conn.execute(
            "ALTER TABLE reference_board_items ADD COLUMN flip_x INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("Failed to add reference_board_items.flip_x: {error}"))?;
    }
    if !table_has_column(conn, "reference_board_items", "flip_y")? {
        conn.execute(
            "ALTER TABLE reference_board_items ADD COLUMN flip_y INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("Failed to add reference_board_items.flip_y: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_user_folder_sort_order(conn: &Connection) -> Result<(), String> {
    let has_sort_order = conn
        .prepare("PRAGMA table_info(user_folders)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| format!("读取文件夹表结构失败：{error}"))?
        .iter()
        .any(|column| column == "sort_order");

    if !has_sort_order {
        conn.execute(
            "ALTER TABLE user_folders ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| format!("升级文件夹排序字段失败：{error}"))?;
    }

    Ok(())
}

#[cfg(test)]
pub(super) fn ensure_user_folder_source_metadata(conn: &Connection) -> Result<(), String> {
    if !table_has_column(conn, "user_folders", "source_kind")? {
        conn.execute(
            "ALTER TABLE user_folders ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'manual'",
            [],
        )
        .map_err(|error| format!("Failed to add user_folders.source_kind: {error}"))?;
    }
    if !table_has_column(conn, "user_folders", "source_path")? {
        conn.execute("ALTER TABLE user_folders ADD COLUMN source_path TEXT", [])
            .map_err(|error| format!("Failed to add user_folders.source_path: {error}"))?;
    }
    let has_source_index = conn
        .prepare("PRAGMA index_list(user_folders)")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))
                .ok()
                .map(|rows| {
                    rows.filter_map(Result::ok)
                        .any(|name| name == "idx_user_folders_source_path")
                })
        })
        .unwrap_or(false);
    if has_source_index {
        return Ok(());
    }
    conn.execute_batch(
        "
        UPDATE user_folders
        SET source_kind = 'manual'
        WHERE source_kind IS NULL OR TRIM(source_kind) = '';

        CREATE UNIQUE INDEX IF NOT EXISTS idx_user_folders_source_path
          ON user_folders(source_path)
          WHERE source_path IS NOT NULL;
        ",
    )
    .map_err(|error| format!("Failed to finalize user folder source metadata: {error}"))?;
    Ok(())
}

pub(super) fn load_store(conn: &Connection) -> Result<LibraryStore, String> {
    let started = Instant::now();
    let t0 = Instant::now();
    let folders = load_folders(conn)?;
    let folders_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let total_image_count = count_gallery_images(conn, Some(ACTIVE_GALLERY_IMAGE_WHERE))?;
    let large_library_mode = total_image_count > LARGE_LIBRARY_IMAGE_THRESHOLD;
    let image_limit = LARGE_LIBRARY_INITIAL_IMAGE_LIMIT;
    let images = load_images_page(conn, Some(ACTIVE_GALLERY_IMAGE_WHERE), 0, image_limit)?;
    let images_ms = t1.elapsed().as_millis();

    let t2 = Instant::now();
    let user_folders = load_user_folders(conn)?;
    let user_folders_ms = t2.elapsed().as_millis();

    let t3 = Instant::now();
    let image_folders = Vec::new();
    let image_folders_ms = t3.elapsed().as_millis();

    let t4 = Instant::now();
    let reference_board_folders = Vec::new();
    let reference_board_folders_ms = t4.elapsed().as_millis();

    let t5 = Instant::now();
    let reference_boards = Vec::new();
    let reference_boards_ms = t5.elapsed().as_millis();

    let t6 = Instant::now();
    let reference_board_items = Vec::new();
    let reference_board_items_ms = t6.elapsed().as_millis();

    eprintln!(
        "[startup-prof] load_store folders_ms={} images_ms={} user_folders_ms={} image_folders_ms={} ref_folders_ms={} ref_boards_ms={} ref_items_ms={} total_ms={} counts=folders:{} images:{}/{} large_mode:{} user_folders:{} image_folders:{} ref_folders:{} ref_boards:{} ref_items:{}",
        folders_ms,
        images_ms,
        user_folders_ms,
        image_folders_ms,
        reference_board_folders_ms,
        reference_boards_ms,
        reference_board_items_ms,
        started.elapsed().as_millis(),
        folders.len(),
        images.len(),
        total_image_count,
        large_library_mode,
        user_folders.len(),
        image_folders.len(),
        reference_board_folders.len(),
        reference_boards.len(),
        reference_board_items.len(),
    );

    Ok(LibraryStore {
        folders,
        images,
        total_image_count,
        large_library_mode,
        image_offset: 0,
        image_limit,
        user_folders,
        image_folders,
        reference_board_folders,
        reference_boards,
        reference_board_items,
    })
}

pub(super) fn load_folders(conn: &Connection) -> Result<Vec<LibraryFolder>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, path, added_at, last_scanned_at
            FROM folders
            WHERE COALESCE(hidden, 0) = 0
            ORDER BY added_at DESC, path ASC
            ",
        )
        .map_err(|error| format!("读取图库文件夹失败：{error}"))?;

    let folders = stmt
        .query_map([], |row| {
            Ok(LibraryFolder {
                id: row.get(0)?,
                path: row.get(1)?,
                added_at: row.get(2)?,
                last_scanned_at: row.get(3)?,
            })
        })
        .map_err(|error| format!("读取图库文件夹失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取图库文件夹失败：{error}"))?;

    Ok(folders)
}

pub(super) fn load_images(conn: &Connection) -> Result<Vec<GalleryImage>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT
              i.id, i.path, t.thumb_path, i.file_name, i.ext, i.width, i.height, i.file_size, i.modified_at,
              i.imported_at, i.folder_id, i.missing, i.trashed, i.is_favorite, i.source
            FROM images i
            LEFT JOIN image_thumbnails t ON t.image_id = i.id
            ORDER BY modified_at DESC, path ASC
            ",
        )
        .map_err(|error| format!("读取图片索引失败：{error}"))?;

    let images = stmt
        .query_map([], |row| {
            Ok(GalleryImage {
                id: row.get(0)?,
                path: row.get(1)?,
                thumbnail_path: row.get(2)?,
                file_name: row.get(3)?,
                ext: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                file_size: row.get(7)?,
                modified_at: row.get(8)?,
                imported_at: row.get(9)?,
                folder_id: row.get(10)?,
                missing: row.get::<_, i64>(11)? != 0,
                trashed: row.get::<_, i64>(12)? != 0,
                is_favorite: row.get::<_, i64>(13)? != 0,
                source: row.get(14)?,
            })
        })
        .map_err(|error| format!("读取图片索引失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取图片索引失败：{error}"))?;

    Ok(images)
}

pub(super) fn map_gallery_image_page_row(row: &Row<'_>) -> rusqlite::Result<GalleryImage> {
    Ok(GalleryImage {
        id: row.get(0)?,
        path: row.get(1)?,
        thumbnail_path: row.get(2)?,
        file_name: row.get(3)?,
        ext: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        file_size: row.get(7)?,
        modified_at: row.get(8)?,
        imported_at: row.get(9)?,
        folder_id: row.get(10)?,
        missing: row.get::<_, i64>(11)? != 0,
        trashed: row.get::<_, i64>(12)? != 0,
        is_favorite: row.get::<_, i64>(13)? != 0,
        source: row.get(14)?,
    })
}

pub(super) fn count_gallery_images(
    conn: &Connection,
    where_sql: Option<&str>,
) -> Result<i64, String> {
    count_gallery_images_with_values(conn, where_sql, Vec::new())
}

pub(super) fn count_gallery_images_with_values(
    conn: &Connection,
    where_sql: Option<&str>,
    values: Vec<Value>,
) -> Result<i64, String> {
    let sql = match where_sql {
        Some(where_sql) if !where_sql.trim().is_empty() => {
            format!("SELECT COUNT(*) FROM images i WHERE {where_sql}")
        }
        _ => "SELECT COUNT(*) FROM images i".to_string(),
    };
    conn.query_row(&sql, params_from_iter(values), |row| row.get(0))
        .map_err(|error| format!("Failed to count gallery images: {error}"))
}

pub(super) fn load_images_page(
    conn: &Connection,
    where_sql: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<GalleryImage>, String> {
    load_images_page_with_values(conn, where_sql, Vec::new(), offset, limit)
}

pub(super) fn load_images_page_with_values(
    conn: &Connection,
    where_sql: Option<&str>,
    values: Vec<Value>,
    offset: i64,
    limit: i64,
) -> Result<Vec<GalleryImage>, String> {
    let where_clause = match where_sql {
        Some(where_sql) if !where_sql.trim().is_empty() => format!("WHERE {where_sql}"),
        _ => String::new(),
    };
    let limit_param_index = values.len() + 1;
    let offset_param_index = values.len() + 2;
    let sql = format!(
        "
        SELECT
          i.id, i.path, t.thumb_path, i.file_name, i.ext, i.width, i.height, i.file_size, i.modified_at,
          i.imported_at, i.folder_id, i.missing, i.trashed, i.is_favorite, i.source
        FROM images i
        LEFT JOIN image_thumbnails t ON t.image_id = i.id
        {where_clause}
        ORDER BY i.modified_at DESC, i.path ASC
        LIMIT ?{limit_param_index} OFFSET ?{offset_param_index}
        "
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("Failed to prepare gallery image page query: {error}"))?;
    let mut params = values;
    params.push(Value::Integer(limit.max(0)));
    params.push(Value::Integer(offset.max(0)));
    let images = stmt
        .query_map(params_from_iter(params), map_gallery_image_page_row)
        .map_err(|error| format!("Failed to query gallery image page: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect gallery image page: {error}"))?;
    Ok(images)
}

pub(super) fn load_user_folders(conn: &Connection) -> Result<Vec<UserFolder>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, parent_id, name, sort_order, created_at, updated_at
            FROM user_folders
            ORDER BY parent_id IS NOT NULL, parent_id, sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取文件夹失败：{error}"))?;

    let folders = stmt
        .query_map([], |row| {
            Ok(UserFolder {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                sort_order: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|error| format!("读取文件夹失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取文件夹失败：{error}"))?;

    Ok(folders)
}

pub(super) fn load_image_folder_assignments(
    conn: &Connection,
) -> Result<Vec<ImageFolderAssignment>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT image_id, folder_id
            FROM image_user_folders
            ORDER BY assigned_at DESC
            ",
        )
        .map_err(|error| format!("读取图片文件夹关系失败：{error}"))?;

    let assignments = stmt
        .query_map([], |row| {
            Ok(ImageFolderAssignment {
                image_id: row.get(0)?,
                folder_id: row.get(1)?,
            })
        })
        .map_err(|error| format!("读取图片文件夹关系失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取图片文件夹关系失败：{error}"))?;

    Ok(assignments)
}

pub(super) fn load_reference_board_folders(
    conn: &Connection,
) -> Result<Vec<ReferenceBoardFolder>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, name, sort_order, created_at, updated_at
            FROM reference_board_folders
            ORDER BY sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取参考板文件夹失败：{error}"))?;

    let folders = stmt
        .query_map([], |row| {
            Ok(ReferenceBoardFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_order: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|error| format!("读取参考板文件夹失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板文件夹失败：{error}"))?;

    Ok(folders)
}

pub(super) fn load_reference_boards(conn: &Connection) -> Result<Vec<ReferenceBoard>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, folder_id, name, sort_order, created_at, updated_at
            FROM reference_boards
            ORDER BY folder_id IS NOT NULL, folder_id, sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取参考板失败：{error}"))?;

    let boards = stmt
        .query_map([], |row| {
            Ok(ReferenceBoard {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                name: row.get(2)?,
                sort_order: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|error| format!("读取参考板失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板失败：{error}"))?;

    Ok(boards)
}

pub(super) fn load_reference_board_items(
    conn: &Connection,
) -> Result<Vec<ReferenceBoardItem>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
            FROM reference_board_items
            ORDER BY board_id, z_index, id
            ",
        )
        .map_err(|error| format!("读取参考板图片失败：{error}"))?;

    let items = stmt
        .query_map([], |row| {
            Ok(ReferenceBoardItem {
                id: row.get(0)?,
                board_id: row.get(1)?,
                image_id: row.get(2)?,
                x: row.get(3)?,
                y: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                rotation: row.get(7)?,
                flip_x: row.get::<_, i64>(8)? != 0,
                flip_y: row.get::<_, i64>(9)? != 0,
                z_index: row.get(10)?,
                created_at: row.get(11)?,
            })
        })
        .map_err(|error| format!("读取参考板图片失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板图片失败：{error}"))?;

    Ok(items)
}

pub(super) fn load_reference_board_item(
    conn: &Connection,
    item_id: i64,
) -> Result<ReferenceBoardItem, String> {
    conn.query_row(
        "
        SELECT id, board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        FROM reference_board_items
        WHERE id = ?1
        ",
        params![item_id],
        |row| {
            Ok(ReferenceBoardItem {
                id: row.get(0)?,
                board_id: row.get(1)?,
                image_id: row.get(2)?,
                x: row.get(3)?,
                y: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                rotation: row.get(7)?,
                flip_x: row.get::<_, i64>(8)? != 0,
                flip_y: row.get::<_, i64>(9)? != 0,
                z_index: row.get(10)?,
                created_at: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("读取参考图失败：{error}"))?
    .ok_or_else(|| "找不到参考图".to_string())
}

pub(super) fn load_reference_board_items_for_board(
    conn: &Connection,
    board_id: i64,
) -> Result<Vec<ReferenceBoardItem>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
            FROM reference_board_items
            WHERE board_id = ?1
            ORDER BY z_index, id
            ",
        )
        .map_err(|error| format!("读取参考板图片失败：{error}"))?;
    let items = stmt
        .query_map(params![board_id], |row| {
            Ok(ReferenceBoardItem {
                id: row.get(0)?,
                board_id: row.get(1)?,
                image_id: row.get(2)?,
                x: row.get(3)?,
                y: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                rotation: row.get(7)?,
                flip_x: row.get::<_, i64>(8)? != 0,
                flip_y: row.get::<_, i64>(9)? != 0,
                z_index: row.get(10)?,
                created_at: row.get(11)?,
            })
        })
        .map_err(|error| format!("读取参考板图片失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板图片失败：{error}"))?;
    Ok(items)
}

pub(super) fn load_image_record(conn: &Connection, image_id: &str) -> Result<GalleryImage, String> {
    conn.query_row(
        "
        SELECT
          i.id, i.path, t.thumb_path, i.file_name, i.ext, i.width, i.height, i.file_size, i.modified_at,
          i.imported_at, i.folder_id, i.missing, i.trashed, i.is_favorite, i.source
        FROM images i
        LEFT JOIN image_thumbnails t ON t.image_id = i.id
        WHERE i.id = ?1
        ",
        params![image_id],
        |row| {
            Ok(GalleryImage {
                id: row.get(0)?,
                path: row.get(1)?,
                thumbnail_path: row.get(2)?,
                file_name: row.get(3)?,
                ext: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                file_size: row.get(7)?,
                modified_at: row.get(8)?,
                imported_at: row.get(9)?,
                folder_id: row.get(10)?,
                missing: row.get::<_, i64>(11)? != 0,
                trashed: row.get::<_, i64>(12)? != 0,
                is_favorite: row.get::<_, i64>(13)? != 0,
                source: row.get(14)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("读取图片索引失败：{error}"))?
    .ok_or_else(|| "找不到图片索引".to_string())
}

pub(super) fn next_reference_board_z_index(
    conn: &Connection,
    board_id: i64,
) -> Result<i64, String> {
    conn.query_row(
        "
        SELECT COALESCE(MAX(z_index), -1) + 1
        FROM reference_board_items
        WHERE board_id = ?1
        ",
        params![board_id],
        |row| row.get(0),
    )
    .map_err(|error| format!("读取参考板层级失败：{error}"))
}

pub(super) fn reference_board_folder_id(
    conn: &Connection,
    board_id: i64,
) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT folder_id FROM reference_boards WHERE id = ?1",
        params![board_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("读取参考板文件夹失败：{error}"))?
    .ok_or_else(|| "找不到参考板".to_string())
}

pub(super) fn load_reference_board_sibling_ids(
    conn: &Connection,
    folder_id: Option<i64>,
) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id
            FROM reference_boards
            WHERE folder_id IS ?1
            ORDER BY sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取参考板排序失败：{error}"))?;
    let board_ids = stmt
        .query_map(params![folder_id], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("读取参考板排序失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板排序失败：{error}"))?;
    Ok(board_ids)
}

pub(super) fn load_reference_board_folder_ids(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id
            FROM reference_board_folders
            ORDER BY sort_order, name COLLATE NOCASE, id
            ",
        )
        .map_err(|error| format!("读取参考板文件夹排序失败：{error}"))?;
    let folder_ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("读取参考板文件夹排序失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取参考板文件夹排序失败：{error}"))?;
    Ok(folder_ids)
}

pub(super) fn copy_image_to_destination(source: &str, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建导出目录失败：{error}"))?;
    }
    fs::copy(source, destination).map_err(|error| format!("导出参考图失败：{error}"))?;
    Ok(())
}

pub(super) fn unique_file_path(folder_path: &Path, file_name: &str) -> PathBuf {
    let base_name = Path::new(file_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "reference".to_string());
    let extension = Path::new(file_name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
        .unwrap_or_else(|| "png".to_string());
    let mut candidate = folder_path.join(format!("{base_name}.{extension}"));
    let mut suffix = 1;
    while candidate.exists() {
        candidate = folder_path.join(format!("{base_name}-{suffix}.{extension}"));
        suffix += 1;
    }
    candidate
}
