use super::*;

pub fn create_user_tag_folder(
    name: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_name = name.trim().to_string();
    if normalized_name.is_empty() {
        return Err("Folder name cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    let now = now_ms();
    let sort_order = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM user_tag_folders",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Failed to query user tag folder sort order: {error}"))?;
    conn.execute(
        "
        INSERT INTO user_tag_folders (name, sort_order, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?3)
        ",
        params![normalized_name, sort_order, now],
    )
    .map_err(|error| format!("Failed to create user tag folder: {error}"))?;
    load_tag_management_state(&conn)
}

pub fn create_user_custom_tag(
    tag_text: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_tag = tag_text.trim().to_string();
    if normalized_tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    upsert_user_custom_tag(&conn, &normalized_tag)?;
    load_tag_management_state(&conn)
}

pub fn delete_user_custom_tag(
    tag_text: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_tag = tag_text.trim().to_string();
    if normalized_tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    let mut conn = open_database(&state.database_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open user custom tag delete transaction: {error}"))?;
    tx.execute(
        "DELETE FROM image_user_custom_tags WHERE tag_text = ?1",
        params![normalized_tag.clone()],
    )
    .map_err(|error| format!("Failed to delete image custom tag rows: {error}"))?;
    tx.execute(
        "DELETE FROM user_tag_folder_members WHERE tag_text = ?1",
        params![normalized_tag.clone()],
    )
    .map_err(|error| format!("Failed to delete tag folder member rows: {error}"))?;
    tx.execute(
        "DELETE FROM user_custom_tags WHERE tag_text = ?1",
        params![normalized_tag],
    )
    .map_err(|error| format!("Failed to delete user custom tag row: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Failed to commit user custom tag delete transaction: {error}"))?;
    load_tag_management_state(&conn)
}

pub fn rename_user_tag_folder(
    folder_id: i64,
    name: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_name = name.trim().to_string();
    if normalized_name.is_empty() {
        return Err("Folder name cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    let now = now_ms();
    let updated = conn
        .execute(
            "
            UPDATE user_tag_folders
            SET name = ?1, updated_at = ?2
            WHERE id = ?3
            ",
            params![normalized_name, now, folder_id],
        )
        .map_err(|error| format!("Failed to rename user tag folder: {error}"))?;
    if updated == 0 {
        return Err("User tag folder not found".to_string());
    }
    load_tag_management_state(&conn)
}

pub fn delete_user_tag_folder(
    folder_id: i64,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let conn = open_database(&state.database_path)?;
    let deleted = conn
        .execute(
            "DELETE FROM user_tag_folders WHERE id = ?1",
            params![folder_id],
        )
        .map_err(|error| format!("Failed to delete user tag folder: {error}"))?;
    if deleted == 0 {
        return Err("User tag folder not found".to_string());
    }
    load_tag_management_state(&conn)
}

pub fn assign_user_tag_to_folder(
    folder_id: i64,
    tag_text: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_tag = tag_text.trim().to_string();
    if normalized_tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    let folder_exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM user_tag_folders WHERE id = ?1)",
            params![folder_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Failed to verify user tag folder: {error}"))?;
    if folder_exists == 0 {
        return Err("User tag folder not found".to_string());
    }
    let tag_exists = conn
        .query_row(
            "
            SELECT EXISTS(
              SELECT 1 FROM user_custom_tags WHERE tag_text = ?1
              UNION
              SELECT 1 FROM image_user_custom_tags WHERE tag_text = ?1
            )
            ",
            params![normalized_tag.clone()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Failed to verify user tag source: {error}"))?;
    if tag_exists == 0 {
        return Err("Tag not found in custom tags".to_string());
    }
    let now = now_ms();
    conn.execute(
        "
        INSERT INTO user_tag_folder_members (folder_id, tag_text, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?3)
        ON CONFLICT(tag_text) DO UPDATE SET
          folder_id = excluded.folder_id,
          updated_at = excluded.updated_at
        ",
        params![folder_id, normalized_tag, now],
    )
    .map_err(|error| format!("Failed to assign user tag to folder: {error}"))?;
    load_tag_management_state(&conn)
}

pub fn unassign_user_tag_from_folder(
    tag_text: String,
    state: &AppState,
) -> Result<TagManagementState, String> {
    let normalized_tag = tag_text.trim().to_string();
    if normalized_tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    let conn = open_database(&state.database_path)?;
    conn.execute(
        "DELETE FROM user_tag_folder_members WHERE tag_text = ?1",
        params![normalized_tag],
    )
    .map_err(|error| format!("Failed to unassign user tag from folder: {error}"))?;
    load_tag_management_state(&conn)
}
