use super::*;

pub fn create_reference_board_folder(
    name: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入参考板文件夹名称".to_string());
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    let now = now_ms();
    let sort_order = next_sort_order(&conn, "reference_board_folders", None)?;

    conn.execute(
        "
        INSERT INTO reference_board_folders (name, sort_order, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?3)
        ",
        params![name, sort_order, now],
    )
    .map_err(|error| format!("创建参考板文件夹失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn create_reference_board(
    folder_id: Option<i64>,
    name: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入参考板名称".to_string());
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    let now = now_ms();
    let sort_order = next_sort_order(&conn, "reference_boards", folder_id)?;

    conn.execute(
        "
        INSERT INTO reference_boards (folder_id, name, sort_order, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?4)
        ",
        params![folder_id, name, sort_order, now],
    )
    .map_err(|error| format!("创建参考板失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn add_image_to_reference_board(
    image_id: String,
    board_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    let (source_width, source_height) = conn
        .query_row(
            "SELECT width, height FROM images WHERE id = ?1",
            params![image_id],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
        )
        .map_err(|error| format!("读取参考图尺寸失败：{error}"))?;
    let (item_width, item_height) = default_reference_board_item_size(source_width, source_height);
    let next_index = conn
        .query_row(
            "
            SELECT COALESCE(MAX(z_index), -1) + 1
            FROM reference_board_items
            WHERE board_id = ?1
            ",
            params![board_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("读取参考板层级失败：{error}"))?;

    conn.execute(
        "
        INSERT OR IGNORE INTO reference_board_items (
          board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 0, ?7, ?8)
        ",
        params![
            board_id,
            image_id,
            (next_index % 5) as f32 * 28.0,
            (next_index / 5) as f32 * 28.0,
            item_width,
            item_height,
            next_index,
            now_ms()
        ],
    )
    .map_err(|error| format!("添加到参考板失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn paste_image_to_reference_board(
    board_id: i64,
    image_bytes: Vec<u8>,
    mime_type: String,
    x: f32,
    y: f32,
    state: &AppState,
) -> Result<LibraryStore, String> {
    if image_bytes.is_empty() {
        return Err("剪贴板里没有可用的图片数据".to_string());
    }

    let decoded = image::load_from_memory(&image_bytes)
        .map_err(|error| format!("读取剪贴板图片失败：{error}"))?;
    let source_width = decoded.width();
    let source_height = decoded.height();
    let (item_width, item_height) = default_reference_board_item_size(source_width, source_height);
    let extension = clipboard_image_extension(&mime_type);

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let app_data_dir = state
        .database_path
        .parent()
        .ok_or_else(|| "找不到应用数据目录".to_string())?;
    let clipboard_dir = app_data_dir.join("clipboard");
    fs::create_dir_all(&clipboard_dir)
        .map_err(|error| format!("创建剪贴板图片目录失败：{error}"))?;

    let now = now_ms();
    let mut file_path = clipboard_dir.join(format!("clipboard-{now}.{extension}"));
    let mut suffix = 1;
    while file_path.exists() {
        file_path = clipboard_dir.join(format!("clipboard-{now}-{suffix}.{extension}"));
        suffix += 1;
    }
    fs::write(&file_path, &image_bytes).map_err(|error| format!("保存剪贴板图片失败：{error}"))?;

    let tx = conn
        .transaction()
        .map_err(|error| format!("打开参考板粘贴事务失败：{error}"))?;
    let folder_path = clipboard_dir.to_string_lossy().to_string();
    let folder_id = upsert_hidden_folder(&tx, &folder_path, now)?;
    let file_name = file_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("clipboard.{extension}"));
    let image_path = file_path.to_string_lossy().to_string();
    let file_size = image_bytes.len() as i64;

    tx.execute(
        "
        INSERT INTO images (
          id, path, file_name, ext, width, height, file_size, modified_at,
          imported_at, folder_id, missing
        )
        VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, 0)
        ",
        params![
            image_path,
            file_name,
            extension,
            source_width,
            source_height,
            file_size,
            now,
            folder_id,
        ],
    )
    .map_err(|error| format!("保存剪贴板图片索引失败：{error}"))?;
    tx.execute(
        "UPDATE images SET source = 'reference' WHERE id = ?1",
        params![image_path],
    )
    .map_err(|error| format!("标记临时参考图失败：{error}"))?;

    let next_index = tx
        .query_row(
            "
            SELECT COALESCE(MAX(z_index), -1) + 1
            FROM reference_board_items
            WHERE board_id = ?1
            ",
            params![board_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("读取参考板层级失败：{error}"))?;

    tx.execute(
        "
        INSERT INTO reference_board_items (
          board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 0, ?7, ?8)
        ",
        params![
            board_id,
            image_path,
            x,
            y,
            item_width,
            item_height,
            next_index,
            now,
        ],
    )
    .map_err(|error| format!("粘贴到参考板失败：{error}"))?;

    tx.commit()
        .map_err(|error| format!("保存参考板粘贴事务失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn paste_images_to_reference_board(
    board_id: i64,
    images: Vec<ReferenceBoardPasteImageInput>,
    x: f32,
    y: f32,
    stack_offset: f32,
    state: &AppState,
) -> Result<LibraryStore, String> {
    if images.is_empty() {
        return Err("No image data to paste".to_string());
    }

    struct PreparedImage {
        image_bytes: Vec<u8>,
        extension: &'static str,
        source_width: u32,
        source_height: u32,
        item_width: f32,
        item_height: f32,
    }

    let mut prepared_images = Vec::with_capacity(images.len());
    for image in images {
        if image.image_bytes.is_empty() {
            return Err("No image data to paste".to_string());
        }
        let decoded = image::load_from_memory(&image.image_bytes)
            .map_err(|error| format!("Failed to read pasted reference image: {error}"))?;
        let source_width = decoded.width();
        let source_height = decoded.height();
        let (item_width, item_height) =
            default_reference_board_item_size(source_width, source_height);
        prepared_images.push(PreparedImage {
            image_bytes: image.image_bytes,
            extension: clipboard_image_extension(&image.mime_type),
            source_width,
            source_height,
            item_width,
            item_height,
        });
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "Library state is busy, please try again later".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let app_data_dir = state
        .database_path
        .parent()
        .ok_or_else(|| "Failed to resolve app data directory".to_string())?;
    let clipboard_dir = app_data_dir.join("clipboard");
    fs::create_dir_all(&clipboard_dir)
        .map_err(|error| format!("Failed to create reference image directory: {error}"))?;

    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open reference board paste transaction: {error}"))?;
    let now = now_ms();
    let folder_path = clipboard_dir.to_string_lossy().to_string();
    let folder_id = upsert_hidden_folder(&tx, &folder_path, now)?;
    let next_index = tx
        .query_row(
            "
            SELECT COALESCE(MAX(z_index), -1) + 1
            FROM reference_board_items
            WHERE board_id = ?1
            ",
            params![board_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Failed to read reference board z-index: {error}"))?;

    for (index, prepared) in prepared_images.iter().enumerate() {
        let item_time = now + index as i64;
        let mut file_path =
            clipboard_dir.join(format!("clipboard-{now}-{index}.{}", prepared.extension));
        let mut suffix = 1;
        while file_path.exists() {
            file_path = clipboard_dir.join(format!(
                "clipboard-{now}-{index}-{suffix}.{}",
                prepared.extension
            ));
            suffix += 1;
        }
        fs::write(&file_path, &prepared.image_bytes)
            .map_err(|error| format!("Failed to save pasted reference image: {error}"))?;

        let file_name = file_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("clipboard.{}", prepared.extension));
        let image_path = file_path.to_string_lossy().to_string();
        let file_size = prepared.image_bytes.len() as i64;

        tx.execute(
            "
            INSERT INTO images (
              id, path, file_name, ext, width, height, file_size, modified_at,
              imported_at, folder_id, missing
            )
            VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, 0)
            ",
            params![
                image_path,
                file_name,
                prepared.extension,
                prepared.source_width,
                prepared.source_height,
                file_size,
                item_time,
                folder_id,
            ],
        )
        .map_err(|error| format!("Failed to save pasted reference image index: {error}"))?;
        tx.execute(
            "UPDATE images SET source = 'reference' WHERE id = ?1",
            params![image_path],
        )
        .map_err(|error| format!("Failed to mark pasted reference image source: {error}"))?;

        tx.execute(
            "
            INSERT INTO reference_board_items (
              board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 0, ?7, ?8)
            ",
            params![
                board_id,
                image_path,
                x + index as f32 * stack_offset,
                y + index as f32 * stack_offset,
                prepared.item_width,
                prepared.item_height,
                next_index + index as i64,
                item_time,
            ],
        )
        .map_err(|error| format!("Failed to paste image into reference board: {error}"))?;
    }

    tx.commit()
        .map_err(|error| format!("Failed to save reference board pasted images: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}
pub fn rename_reference_board(
    board_id: i64,
    name: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入参考板名称".to_string());
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "UPDATE reference_boards SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now_ms(), board_id],
    )
    .map_err(|error| format!("重命名参考板失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn rename_reference_board_folder(
    folder_id: i64,
    name: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入参考板文件夹名称".to_string());
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "UPDATE reference_board_folders SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now_ms(), folder_id],
    )
    .map_err(|error| format!("重命名参考板文件夹失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn reorder_reference_board_folder(
    folder_id: i64,
    target_folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    if folder_id == target_folder_id {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开参考板文件夹排序事务失败：{error}"))?;
    let folder_ids = load_reference_board_folder_ids(&tx)?;
    let Some(from_index) = folder_ids.iter().position(|id| *id == folder_id) else {
        return Err("找不到要排序的参考板文件夹".to_string());
    };
    let Some(to_index) = folder_ids.iter().position(|id| *id == target_folder_id) else {
        return Err("找不到目标参考板文件夹".to_string());
    };

    let mut reordered = folder_ids;
    let moved = reordered.remove(from_index);
    reordered.insert(to_index, moved);
    for (index, id) in reordered.iter().enumerate() {
        tx.execute(
            "UPDATE reference_board_folders SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![index as i64, now_ms(), id],
        )
        .map_err(|error| format!("保存参考板文件夹排序失败：{error}"))?;
    }
    tx.commit()
        .map_err(|error| format!("保存参考板文件夹排序事务失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn move_reference_board_to_folder(
    board_id: i64,
    folder_id: Option<i64>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    let sort_order = next_sort_order(&conn, "reference_boards", folder_id)?;
    conn.execute(
        "
        UPDATE reference_boards
        SET folder_id = ?1, sort_order = ?2, updated_at = ?3
        WHERE id = ?4
        ",
        params![folder_id, sort_order, now_ms(), board_id],
    )
    .map_err(|error| format!("移动参考板失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn reorder_reference_board(
    board_id: i64,
    target_board_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    if board_id == target_board_id {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let target_folder = reference_board_folder_id(&conn, target_board_id)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开参考板排序事务失败：{error}"))?;
    tx.execute(
        "UPDATE reference_boards SET folder_id = ?1 WHERE id = ?2",
        params![target_folder, board_id],
    )
    .map_err(|error| format!("移动参考板失败：{error}"))?;
    let sibling_ids = load_reference_board_sibling_ids(&tx, target_folder)?;
    let Some(from_index) = sibling_ids.iter().position(|id| *id == board_id) else {
        return Err("找不到要排序的参考板".to_string());
    };
    let Some(to_index) = sibling_ids.iter().position(|id| *id == target_board_id) else {
        return Err("找不到目标参考板".to_string());
    };

    let mut reordered = sibling_ids;
    let moved = reordered.remove(from_index);
    reordered.insert(to_index, moved);
    for (index, id) in reordered.iter().enumerate() {
        tx.execute(
            "UPDATE reference_boards SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![index as i64, now_ms(), id],
        )
        .map_err(|error| format!("保存参考板排序失败：{error}"))?;
    }
    tx.commit()
        .map_err(|error| format!("保存参考板排序事务失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn delete_reference_board(board_id: i64, state: &AppState) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    conn.execute(
        "DELETE FROM reference_boards WHERE id = ?1",
        params![board_id],
    )
    .map_err(|error| format!("删除参考板失败：{error}"))?;

    cleanup_orphan_reference_images(&conn)?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn delete_reference_board_folder(
    folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    conn.execute(
        "DELETE FROM reference_board_folders WHERE id = ?1",
        params![folder_id],
    )
    .map_err(|error| format!("删除参考板文件夹失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn remove_reference_board_item(item_id: i64, state: &AppState) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "DELETE FROM reference_board_items WHERE id = ?1",
        params![item_id],
    )
    .map_err(|error| format!("移除参考图失败：{error}"))?;

    cleanup_orphan_reference_images(&conn)?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn update_reference_board_item_layout(
    item_id: i64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
    flip_x: bool,
    flip_y: bool,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "
        UPDATE reference_board_items
        SET x = ?1, y = ?2, width = ?3, height = ?4, rotation = ?5, flip_x = ?6, flip_y = ?7
        WHERE id = ?8
        ",
        params![
            x,
            y,
            width.max(48.0),
            height.max(48.0),
            rotation,
            if flip_x { 1 } else { 0 },
            if flip_y { 1 } else { 0 },
            item_id
        ],
    )
    .map_err(|error| format!("保存参考图布局失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn bring_reference_board_item_to_front(
    item_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "鍥惧簱鐘舵€佽鍗犵敤锛岃绋嶅悗鍐嶈瘯".to_string())?;
    let conn = open_database(&state.database_path)?;
    let item = load_reference_board_item(&conn, item_id)?;
    let next_index = next_reference_board_z_index(&conn, item.board_id)?;
    conn.execute(
        "
        UPDATE reference_board_items
        SET z_index = ?1
        WHERE id = ?2
        ",
        params![next_index, item_id],
    )
    .map_err(|error| format!("缃簬鏈€鍓嶅眰澶辫触锛歿{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn duplicate_reference_board_item(
    item_id: i64,
    x: Option<f32>,
    y: Option<f32>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    let item = load_reference_board_item(&conn, item_id)?;
    let next_index = next_reference_board_z_index(&conn, item.board_id)?;
    conn.execute(
        "
        INSERT INTO reference_board_items (
          board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ",
        params![
            item.board_id,
            item.image_id,
            x.unwrap_or(item.x + 28.0),
            y.unwrap_or(item.y + 28.0),
            item.width,
            item.height,
            item.rotation,
            if item.flip_x { 1 } else { 0 },
            if item.flip_y { 1 } else { 0 },
            next_index,
            now_ms(),
        ],
    )
    .map_err(|error| format!("复制参考图失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn restore_reference_board_item(
    board_id: i64,
    image_id: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
    flip_x: bool,
    flip_y: bool,
    z_index: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "
        INSERT INTO reference_board_items (
          board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ",
        params![
            board_id,
            image_id,
            x,
            y,
            width.max(48.0),
            height.max(48.0),
            rotation,
            if flip_x { 1 } else { 0 },
            if flip_y { 1 } else { 0 },
            z_index,
            now_ms(),
        ],
    )
    .map_err(|error| format!("恢复参考图失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn export_reference_board_item_from_state(
    item_id: i64,
    destination: String,
    state: &AppState,
) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;
    let item = load_reference_board_item(&conn, item_id)?;
    copy_image_to_destination(&item.image_id, Path::new(&destination))
}

pub fn export_gallery_image_from_state(
    image_id: String,
    destination: String,
    state: &AppState,
) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;
    let image = load_image_record(&conn, &image_id)?;
    copy_image_to_destination(&image.path, Path::new(&destination))
}

pub fn import_reference_board_item_to_library(
    item_id: i64,
    folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let item = load_reference_board_item(&conn, item_id)?;
    let image = load_image_record(&conn, &item.image_id)?;
    if image.source != "reference" {
        let store = load_store(&conn)?;
        *library = Some(store.clone());
        return Ok(store);
    }
    let folder_path = conn
        .query_row(
            "SELECT path FROM folders WHERE id = ?1 AND COALESCE(hidden, 0) = 0",
            params![folder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("读取图库文件夹失败：{error}"))?
        .ok_or_else(|| "找不到目标图库文件夹".to_string())?;
    let target_path = unique_file_path(Path::new(&folder_path), &image.file_name);

    fs::copy(&image.path, &target_path)
        .map_err(|error| format!("复制到图库文件夹失败：{error}"))?;
    let metadata =
        fs::metadata(&target_path).map_err(|error| format!("读取图库图片信息失败：{error}"))?;
    let target_path_text = target_path.to_string_lossy().to_string();
    let now = now_ms();
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开加入图库事务失败：{error}"))?;
    tx.execute(
        "
        INSERT INTO images (
          id, path, file_name, ext, width, height, file_size, modified_at,
          imported_at, folder_id, missing, source
        )
        VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 'library')
        ",
        params![
            target_path_text,
            target_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| image.file_name.clone()),
            image.ext.clone(),
            image.width,
            image.height,
            metadata.len() as i64,
            metadata.modified().ok().map(system_time_ms).unwrap_or(now),
            now,
            folder_id,
        ],
    )
    .map_err(|error| format!("保存图库图片索引失败：{error}"))?;
    tx.execute(
        "UPDATE reference_board_items SET image_id = ?1 WHERE id = ?2",
        params![target_path_text, item_id],
    )
    .map_err(|error| format!("更新参考图索引失败：{error}"))?;
    tx.commit()
        .map_err(|error| format!("保存加入图库事务失败：{error}"))?;
    cleanup_orphan_reference_images(&conn)?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn auto_arrange_reference_board(
    board_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let items = load_reference_board_items_for_board(&conn, board_id)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开自动排列事务失败：{error}"))?;
    let total_area: f32 = items.iter().map(|item| item.width * item.height).sum();
    let max_item_width: f32 = items.iter().map(|item| item.width).fold(0.0, f32::max);
    let max_row_width = (total_area.sqrt() * 1.18).max(max_item_width);
    let mut x = 0.0;
    let mut y = 0.0;
    let mut row_height: f32 = 0.0;
    let gap = 24.0;
    for item in items {
        if x > 0.0 && x + item.width > max_row_width {
            x = 0.0;
            y += row_height + gap;
            row_height = 0.0;
        }
        tx.execute(
            "
            UPDATE reference_board_items
            SET x = ?1, y = ?2, rotation = 0
            WHERE id = ?3
            ",
            params![x, y, item.id],
        )
        .map_err(|error| format!("自动排列参考图失败：{error}"))?;
        x += item.width + gap;
        row_height = row_height.max(item.height);
    }
    tx.commit()
        .map_err(|error| format!("保存自动排列事务失败：{error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}
