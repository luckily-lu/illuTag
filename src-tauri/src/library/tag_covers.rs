//! Small on-demand tag covers. User choices live in app_meta and travel with backups.
use super::*;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TagCover {
    pub tag: String,
    pub image: Option<GalleryImage>,
    pub pinned: bool,
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TagCoverRequest {
    pub tag: String,
    pub image_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoverChoice {
    pk: i64,
    id: String,
    x: f64,
    y: f64,
    zoom: f64,
}

fn tag_key(tag: &str) -> Result<String, String> {
    if tag.trim().is_empty() || tag.len() > 512 {
        return Err("标签无效".into());
    }
    Ok(format!("tag-cover:v1:{}", tag.trim()))
}
fn member(db: &Connection, tag: &str, pk: i64) -> Result<bool, String> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM storage_auto_tags WHERE image_pk=?1 AND model_pk=(SELECT string_pk FROM storage_strings WHERE value=?2) AND tag_pk=(SELECT string_pk FROM storage_strings WHERE value=?3) AND confidence>=0.99)",params![pk,WD_TAGGER_MODEL_NAME,tag],|r|r.get(0)).map_err(|e|e.to_string())
}
fn usable(image: &GalleryImage) -> bool {
    !image.trashed
        && !image.missing
        && image.source != "reference"
        && image.width > 0
        && image.height > 0
        && (image
            .thumbnail_path
            .as_ref()
            .is_some_and(|p| Path::new(p).is_file())
            || Path::new(&image.path).is_file())
}
fn normalise_cover(mut image: GalleryImage) -> Option<GalleryImage> {
    if !usable(&image) {
        return None;
    }
    if image
        .thumbnail_path
        .as_ref()
        .is_some_and(|p| !Path::new(p).is_file())
    {
        image.thumbnail_path = None;
    }
    Some(image)
}
fn auto_key(tag: &str) -> String {
    format!("tag-cover:auto:v2:{tag}")
}

// Only cover references are loaded, never the library's image-ID collection.
// Validate all saved owners in one indexed SQL join. Pins retain ownership while
// recycled so restoring one cannot steal another tag's current cover.
struct CoverRegistry {
    choices: HashMap<String, CoverChoice>,
    discarded: Vec<String>,
}
impl CoverRegistry {
    fn load(db: &Connection) -> Result<Self, String> {
        db.execute_batch("CREATE TEMP TABLE cover_choices(key TEXT PRIMARY KEY,tag TEXT NOT NULL,image_pk INTEGER NOT NULL,public_id TEXT NOT NULL,pinned INTEGER NOT NULL) WITHOUT ROWID; CREATE TEMP TABLE cover_owners(image_pk INTEGER PRIMARY KEY,tag TEXT NOT NULL) WITHOUT ROWID;").map_err(|e|e.to_string())?;
        let mut stmt = db.prepare("SELECT key,value FROM app_meta WHERE key GLOB 'tag-cover:v1:*' OR key GLOB 'tag-cover:auto:v2:*'").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut saved = HashMap::new();
        let mut insert = db
            .prepare("INSERT INTO cover_choices VALUES(?1,?2,?3,?4,?5)")
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (key, raw) = row.map_err(|e| e.to_string())?;
            let Ok(choice) = serde_json::from_str::<CoverChoice>(&raw) else {
                continue;
            };
            let (tag, pinned) = if let Some(tag) = key.strip_prefix("tag-cover:v1:") {
                (tag, true)
            } else {
                (key.strip_prefix("tag-cover:auto:v2:").unwrap(), false)
            };
            insert
                .execute(params![key, tag, choice.pk, choice.id, pinned])
                .map_err(|e| e.to_string())?;
            saved.insert(key, choice);
        }
        // Legacy duplicate pins are resolved deterministically before auto covers.
        let mut stmt = db.prepare("SELECT c.key,c.tag,c.image_pk,i.trashed=0 AND i.missing=0 AND i.source<>'reference' FROM cover_choices c JOIN images i ON i.image_pk=c.image_pk AND i.id=c.public_id WHERE (c.pinned=1 OR (i.trashed=0 AND i.missing=0 AND i.source<>'reference')) AND EXISTS(SELECT 1 FROM storage_auto_tags a WHERE a.image_pk=c.image_pk AND a.model_pk=(SELECT string_pk FROM storage_strings WHERE value=?1) AND a.tag_pk=(SELECT string_pk FROM storage_strings WHERE value=c.tag) AND a.confidence>=0.99) ORDER BY c.pinned DESC,c.key").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([WD_TAGGER_MODEL_NAME], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut choices = HashMap::new();
        let mut owners: HashMap<i64, String> = HashMap::new();
        let mut active_pins = HashSet::new();
        for row in rows {
            let (key, tag, pk, active) = row.map_err(|e| e.to_string())?;
            let pinned = key.starts_with("tag-cover:v1:");
            if !pinned && active_pins.contains(&tag) {
                continue;
            }
            if owners.get(&pk).is_some_and(|owner| owner != &tag) {
                continue;
            }
            if pinned && active {
                active_pins.insert(tag.clone());
            }
            owners.insert(pk, tag);
            choices.insert(key.clone(), saved.remove(&key).unwrap());
        }
        let mut insert = db
            .prepare("INSERT INTO cover_owners VALUES(?1,?2)")
            .map_err(|e| e.to_string())?;
        for (pk, tag) in owners {
            insert
                .execute(params![pk, tag])
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            choices,
            discarded: saved.into_keys().collect(),
        })
    }
    fn discard_invalid(&self, db: &Connection) -> Result<(), String> {
        for key in &self.discarded {
            db.execute("DELETE FROM app_meta WHERE key=?1", [key])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn reserve(
        &mut self,
        db: &Connection,
        key: String,
        tag: &str,
        choice: CoverChoice,
        persist: bool,
    ) -> Result<(), String> {
        if let Some(old) = self.choices.insert(key.clone(), choice.clone()) {
            if !self.choices.values().any(|c| c.pk == old.pk) {
                db.execute("DELETE FROM cover_owners WHERE image_pk=?1", [old.pk])
                    .map_err(|e| e.to_string())?;
            }
        }
        db.execute("INSERT INTO cover_owners VALUES(?1,?2) ON CONFLICT(image_pk) DO UPDATE SET tag=excluded.tag",params![choice.pk,tag]).map_err(|e|e.to_string())?;
        if persist {
            write_app_meta_value(
                db,
                &key,
                &serde_json::to_string(&choice).map_err(|e| e.to_string())?,
            )?;
        }
        Ok(())
    }
}
fn for_tag(
    db: &Connection,
    registry: &mut CoverRegistry,
    tag: &str,
    persist: bool,
) -> Result<TagCover, String> {
    let key = tag_key(tag)?;
    for (key, pinned) in [(key, true), (auto_key(tag), false)] {
        if let Some(choice) = registry.choices.get(&key) {
            if let Some(image) = load_image_record(db, &choice.id)
                .ok()
                .and_then(normalise_cover)
            {
                return Ok(TagCover {
                    tag: tag.into(),
                    image: Some(image),
                    pinned,
                    x: choice.x,
                    y: choice.y,
                    zoom: choice.zoom,
                });
            }
        }
    }
    // A new allocation must be persisted to remain unique across page requests.
    if !persist {
        return Ok(TagCover {
            tag: tag.into(),
            image: None,
            pinned: false,
            x: 50.0,
            y: 50.0,
            zoom: 1.0,
        });
    }
    // Exclude assigned pictures and <99% tags before LIMIT, so earlier occupied
    // rows cannot hide eligible images further along this tag's reverse index.
    let mut stmt=db.prepare("SELECT a.image_pk,MAX(a.confidence) FROM storage_auto_tags a INDEXED BY storage_auto_tags_reverse WHERE a.model_pk=(SELECT string_pk FROM storage_strings WHERE value=?1) AND a.tag_pk=(SELECT string_pk FROM storage_strings WHERE value=?2) AND a.confidence>=0.99 AND NOT EXISTS(SELECT 1 FROM cover_owners o WHERE o.image_pk=a.image_pk AND o.tag<>?2) AND EXISTS(SELECT 1 FROM images i WHERE i.image_pk=a.image_pk AND i.trashed=0 AND i.missing=0 AND i.source<>'reference') GROUP BY a.image_pk ORDER BY a.image_pk LIMIT 96").map_err(|e|e.to_string())?;
    let pool = stmt
        .query_map(params![WD_TAGGER_MODEL_NAME, tag], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut ranked = Vec::new();
    for (pk, confidence) in pool {
        let row:Option<(String,i64,i64,bool)>=db.query_row("SELECT id,width,height,is_favorite<>0 FROM images WHERE image_pk=?1 AND trashed=0 AND missing=0 AND source<>'reference'",[pk],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
        if let Some((id, w, h, favorite)) = row {
            if w <= 0 || h <= 0 || w.saturating_mul(h) > 32_000_000 {
                continue;
            }
            let ratio = w as f64 / h as f64;
            let shape = (1.0 - (ratio / (16.0 / 9.0)).ln().abs()).max(-2.0);
            let resolution = ((w as f64 * h as f64) / 1_000_000.0).min(4.0) / 4.0;
            ranked.push((
                confidence * 4.0 + shape + resolution + if favorite { 0.15 } else { 0.0 },
                pk,
                id,
            ));
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let image = ranked
        .into_iter()
        .take(16)
        .filter_map(|(_, pk, id)| {
            load_image_record(db, &id)
                .ok()
                .and_then(normalise_cover)
                .map(|image| (pk, image))
        })
        .next();
    if let Some((pk, image)) = &image {
        registry.reserve(
            db,
            auto_key(tag),
            tag,
            CoverChoice {
                pk: *pk,
                id: image.id.clone(),
                x: 50.0,
                y: 50.0,
                zoom: 1.0,
            },
            persist,
        )?;
    }
    Ok(TagCover {
        tag: tag.into(),
        image: image.map(|(_, image)| image),
        pinned: false,
        x: 50.0,
        y: 50.0,
        zoom: 1.0,
    })
}
pub fn list_tag_covers(tags: Vec<String>, state: &AppState) -> Result<Vec<TagCover>, String> {
    if tags.len() > 24 {
        return Err("每页最多读取24个标签封面".into());
    }
    for tag in &tags {
        tag_key(tag)?;
    }
    let _guard = state.write_lock.lock().map_err(|e| e.to_string())?;
    let db = open_database(&state.database_path)?;
    let persist = !state.read_only && !state.maintenance.load(std::sync::atomic::Ordering::SeqCst);
    db.execute_batch(if persist {
        "BEGIN IMMEDIATE"
    } else {
        "BEGIN DEFERRED"
    })
    .map_err(|e| e.to_string())?;
    let mut registry = CoverRegistry::load(&db)?;
    if persist {
        registry.discard_invalid(&db)?;
    }
    let covers = tags
        .into_iter()
        .map(|tag| for_tag(&db, &mut registry, tag.trim(), persist))
        .collect::<Result<Vec<_>, _>>()?;
    db.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(covers)
}
pub fn save_tag_cover(request: TagCoverRequest, state: &AppState) -> Result<TagCover, String> {
    if state.read_only || state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("图库只读或维护中".into());
    }
    let key = tag_key(&request.tag)?;
    if !request.x.is_finite()
        || !request.y.is_finite()
        || !request.zoom.is_finite()
        || !(0.0..=100.0).contains(&request.x)
        || !(0.0..=100.0).contains(&request.y)
        || !(1.0..=3.0).contains(&request.zoom)
    {
        return Err("封面位置或缩放无效".into());
    }
    let db = open_database(&state.database_path)?;
    db.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let _registry = CoverRegistry::load(&db)?;
    _registry.discard_invalid(&db)?;
    if let Some(id) = request.image_id {
        let image = load_image_record(&db, &id)?;
        let pk: i64 = db
            .query_row("SELECT image_pk FROM images WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if !usable(&image) || !member(&db, &request.tag, pk)? {
            return Err("封面图片必须仍可用，且该标签的模型置信度至少为99%".into());
        }
        let owner: Option<String> = db
            .query_row(
                "SELECT tag FROM cover_owners WHERE image_pk=?1 AND tag<>?2",
                params![pk, request.tag.trim()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(owner) = owner {
            return Err(format!("该图片已用于标签「{owner}」的封面，请选择其他图片"));
        }
        let choice = CoverChoice {
            pk,
            id,
            x: request.x,
            y: request.y,
            zoom: request.zoom,
        };
        write_app_meta_value(
            &db,
            &key,
            &serde_json::to_string(&choice).map_err(|e| e.to_string())?,
        )?;
    } else {
        db.execute("DELETE FROM app_meta WHERE key=?1", [key])
            .map_err(|e| e.to_string())?;
    }
    // Reload ownership after a manual choice or reset; release the old automatic
    // allocation while keeping a temporarily unavailable manual pin restorable.
    db.execute(
        "DELETE FROM app_meta WHERE key=?1",
        [auto_key(request.tag.trim())],
    )
    .map_err(|e| e.to_string())?;
    db.execute_batch("DROP TABLE cover_choices; DROP TABLE cover_owners;")
        .map_err(|e| e.to_string())?;
    let mut registry = CoverRegistry::load(&db)?;
    let cover = for_tag(&db, &mut registry, request.tag.trim(), true)?;
    db.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(cover)
}

pub(super) fn remap_choices(db: &Connection, mappings: &[BackupPathMapping]) -> Result<(), String> {
    let mut stmt = db
        .prepare("SELECT key,value FROM app_meta WHERE key GLOB 'tag-cover:v1:*' OR key GLOB 'tag-cover:auto:v2:*'")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (key, raw) in rows {
        if let Ok(mut choice) = serde_json::from_str::<CoverChoice>(&raw) {
            let next = remap_backup_path(&choice.id, mappings);
            if next != choice.id {
                choice.id = next;
                write_app_meta_value(
                    db,
                    &key,
                    &serde_json::to_string(&choice).map_err(|e| e.to_string())?,
                )?;
            }
        }
    }
    Ok(())
}

/// Paged directly in SQLite; never build the common tag's full image-ID array.
pub fn query_tag_preview(
    tags: Vec<String>,
    offset: i64,
    limit: i64,
    state: &AppState,
) -> Result<QueryPage, String> {
    if tags.len() > 32 {
        return Err("组合条件过多".into());
    }
    let db = open_database(&state.database_path)?;
    let (sql, values) = gallery_search_sql(GallerySearchFilters {
        chinese_tag_ens: tags,
        english_query: String::new(),
        file_name_query: String::new(),
        confidence_min: 0.0,
        confidence_max: 1.0,
        scope: Some("all".into()),
        folder_id: None,
        unclassified_only_parent_folder_id: None,
    });
    let predicate = sql
        .split_once("FROM images")
        .ok_or("标签查询无效")?
        .1
        .rsplit_once(" ORDER BY")
        .ok_or("标签查询排序无效")?
        .0
        .trim()
        .strip_prefix("WHERE")
        .ok_or("标签查询范围无效")?
        .replace("images.", "i.");
    let offset = offset.max(0);
    let limit = limit.clamp(1, 50);
    db.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let count = count_gallery_images_with_values(&db, Some(&predicate), values.clone())?;
    let images = load_images_page_with_values(&db, Some(&predicate), values, offset, limit)?;
    Ok(QueryPage {
        images,
        total_image_count: count,
        offset,
        limit,
        revision: *state
            .startup_cleanup_generation
            .lock()
            .map_err(|e| e.to_string())?,
    })
}

/// Cover editing has a stricter predicate than ordinary tag search. A custom or
/// supplementary tag is not evidence of a 99% model prediction.
pub fn query_tag_cover_candidates(
    tag: String,
    offset: i64,
    limit: i64,
    state: &AppState,
) -> Result<QueryPage, String> {
    tag_key(&tag)?;
    let db = open_database(&state.database_path)?;
    db.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let _registry = CoverRegistry::load(&db)?;
    let predicate = "i.trashed=0 AND i.missing=0 AND i.source<>'reference' AND i.width>0 AND i.height>0 AND i.image_pk IN (SELECT image_pk FROM storage_auto_tags INDEXED BY storage_auto_tags_reverse WHERE model_pk=(SELECT string_pk FROM storage_strings WHERE value=?1) AND tag_pk=(SELECT string_pk FROM storage_strings WHERE value=?2) AND confidence>=0.99) AND NOT EXISTS(SELECT 1 FROM cover_owners o WHERE o.image_pk=i.image_pk AND o.tag<>?2)";
    let values = vec![
        Value::Text(WD_TAGGER_MODEL_NAME.into()),
        Value::Text(tag.trim().into()),
    ];
    let total_image_count = count_gallery_images_with_values(&db, Some(predicate), values.clone())?;
    let offset = offset.max(0);
    let limit = limit.clamp(1, 50);
    let images = load_images_page_with_values(&db, Some(predicate), values, offset, limit)?;
    Ok(QueryPage {
        images,
        total_image_count,
        offset,
        limit,
        revision: *state
            .startup_cleanup_generation
            .lock()
            .map_err(|e| e.to_string())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn covers_require_99_percent_and_have_one_owner_across_pages_and_restarts() {
        let base =
            PathBuf::from(env::var_os("ILLUTAG_TEST_ROOT").expect("explicit isolated test root"));
        assert!(
            base.is_absolute()
                && base
                    .components()
                    .any(|p| p.as_os_str() == "illuTag-refactor-work")
        );
        let root = base.join(format!("cover-policy-{}-{}", std::process::id(), now_ms()));
        assert!(!root.exists());
        fs::create_dir_all(&root).unwrap();
        let state = AppState::new(root.join("illutag.sqlite"), root.clone(), None, None);
        let db = open_database(&state.database_path).unwrap();
        db.execute(
            "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();
        let mut ids = Vec::new();
        for n in 1..=100 {
            let path = root.join(format!("sample-{n}.png"));
            ImageBuffer::from_pixel(64, 40, Rgb([n as u8, 80, 190]))
                .save(&path)
                .unwrap();
            let id = path.to_string_lossy().to_string();
            ids.push(id.clone());
            db.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES(?1,?2,?2,?2,'png',64,40,100,0,0,1)",params![n,id]).unwrap();
            for tag in ["tag_a", "tag_b"] {
                compact::upsert_auto_tag(
                    &db,
                    &id,
                    "general",
                    tag,
                    None,
                    if n == 1 { 0.9899 } else { 0.99 },
                    WD_TAGGER_MODEL_NAME,
                    0,
                )
                .unwrap();
            }
        }
        // A manual annotation cannot bypass the confidence threshold.
        db.execute(
            "INSERT INTO storage_image_user_custom_tags VALUES(1,'manual_only',0,0)",
            [],
        )
        .unwrap();
        let request = |tag: &str, id: String| TagCoverRequest {
            tag: tag.into(),
            image_id: Some(id),
            x: 50.0,
            y: 50.0,
            zoom: 1.0,
        };
        assert!(save_tag_cover(request("tag_a", ids[0].clone()), &state)
            .unwrap_err()
            .contains("99%"));
        assert!(save_tag_cover(request("manual_only", ids[0].clone()), &state).is_err());
        let a = list_tag_covers(vec!["tag_a".into()], &state)
            .unwrap()
            .remove(0);
        assert_eq!(a.image.as_ref().unwrap().id, ids[1]);
        // A different request/page and a fresh AppState still see the reservation.
        let reopened = AppState::new(state.database_path.clone(), root.clone(), None, None);
        let b = list_tag_covers(vec!["tag_b".into()], &reopened)
            .unwrap()
            .remove(0);
        assert_ne!(a.image.as_ref().unwrap().id, b.image.as_ref().unwrap().id);
        assert!(save_tag_cover(request("tag_b", ids[1].clone()), &state)
            .unwrap_err()
            .contains("已用于"));
        let candidates = query_tag_cover_candidates("tag_b".into(), 0, 50, &state).unwrap();
        assert_eq!(candidates.total_image_count, 98);
        assert!(!candidates
            .images
            .iter()
            .any(|i| i.id == ids[0] || i.id == ids[1]));
        assert_eq!(
            query_tag_preview(vec!["tag_b".into()], 0, 50, &state)
                .unwrap()
                .total_image_count,
            100
        );
        assert!(
            list_tag_covers(vec!["manual_only".into()], &state).unwrap()[0]
                .image
                .is_none()
        );
        // Reserve the first 96 eligible rows for other tags: filtering must occur
        // before the bounded automatic pool, not after it.
        for (index, id) in ids.iter().enumerate().take(98).skip(1) {
            let tag = format!("owner-{index}");
            compact::upsert_auto_tag(&db, id, "general", &tag, None, 1.0, WD_TAGGER_MODEL_NAME, 0)
                .unwrap();
            write_app_meta_value(
                &db,
                &tag_key(&tag).unwrap(),
                &serde_json::to_string(&CoverChoice {
                    pk: index as i64 + 1,
                    id: id.clone(),
                    x: 50.0,
                    y: 50.0,
                    zoom: 1.0,
                })
                .unwrap(),
            )
            .unwrap();
        }
        let late = list_tag_covers(vec!["tag_a".into()], &state)
            .unwrap()
            .remove(0);
        assert_eq!(late.image.unwrap().id, ids[98]);
        // Deterministic repair of old duplicate pins, including low-confidence
        // old pins: no grandfathering and no duplicate visible covers.
        let legacy = CoverChoice {
            pk: 100,
            id: ids[99].clone(),
            x: 50.0,
            y: 50.0,
            zoom: 1.0,
        };
        for tag in ["tag_a", "tag_b"] {
            write_app_meta_value(
                &db,
                &tag_key(tag).unwrap(),
                &serde_json::to_string(&legacy).unwrap(),
            )
            .unwrap();
        }
        let both = list_tag_covers(vec!["tag_a".into(), "tag_b".into()], &state).unwrap();
        assert!(both[0].pinned);
        assert_ne!(
            both[0].image.as_ref().unwrap().id,
            both[1].image.as_ref().unwrap().id
        );
        drop(db);
        drop(reopened);
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tag_covers_are_stable_pinned_and_preview_is_paged_without_image_mutation() {
        let base =
            PathBuf::from(env::var_os("ILLUTAG_TEST_ROOT").expect("explicit isolated test root"));
        assert!(
            base.is_absolute()
                && base
                    .components()
                    .any(|p| p.as_os_str() == "illuTag-refactor-work")
        );
        let root = base.join(format!("tag-covers-{}-{}", std::process::id(), now_ms()));
        assert!(!root.exists());
        fs::create_dir_all(&root).unwrap();
        let state = AppState::new(root.join("illutag.sqlite"), root.clone(), None, None);
        let db = open_database(&state.database_path).unwrap();
        db.execute(
            "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();
        let mut ids = Vec::new();
        for n in 1..=3 {
            let file = root.join(format!("cover-{n}.png"));
            ImageBuffer::from_pixel(64, 40, Rgb([20u8, 100, 150]))
                .save(&file)
                .unwrap();
            let id = file.to_string_lossy().to_string();
            ids.push(id.clone());
            db.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES(?1,?2,?2,?3,'png',64,40,100,0,0,1)",params![n,id,format!("cover-{n}.png")]).unwrap();
            if n < 3 {
                compact::upsert_auto_tag(
                    &db,
                    &ids[n as usize - 1],
                    "general",
                    "blue_sky",
                    Some("蓝天"),
                    if n == 1 { 1.0 } else { 0.99 },
                    WD_TAGGER_MODEL_NAME,
                    0,
                )
                .unwrap();
            }
        }
        db.execute(
            "INSERT INTO storage_image_user_custom_tags VALUES(2,'我的标签',0,0)",
            [],
        )
        .unwrap();
        let auto = list_tag_covers(vec!["blue_sky".into()], &state)
            .unwrap()
            .remove(0);
        assert_eq!(auto.image.unwrap().id, ids[0]);
        let request = TagCoverRequest {
            tag: "blue_sky".into(),
            image_id: Some(ids[1].clone()),
            x: 22.0,
            y: 73.0,
            zoom: 1.65,
        };
        let pin = save_tag_cover(request.clone(), &state).unwrap();
        assert!(pin.pinned);
        assert_eq!((pin.x, pin.y, pin.zoom), (22.0, 73.0, 1.65));
        assert_eq!(
            list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0]
                .image
                .as_ref()
                .unwrap()
                .id,
            ids[1]
        );
        let mut invalid = request.clone();
        invalid.image_id = Some(ids[2].clone());
        assert!(save_tag_cover(invalid, &state).is_err());
        let mut invalid = request.clone();
        invalid.x = f64::NAN;
        assert!(save_tag_cover(invalid, &state).is_err());
        let preview = query_tag_preview(vec!["blue_sky".into()], 1, 1, &state).unwrap();
        assert_eq!((preview.total_image_count, preview.images.len()), (2, 1));
        let intersection =
            query_tag_preview(vec!["blue_sky".into(), "我的标签".into()], 0, 8, &state).unwrap();
        assert_eq!(
            (
                intersection.total_image_count,
                intersection.images[0].id.as_str()
            ),
            (1, ids[1].as_str())
        );
        db.execute("UPDATE images SET trashed=1 WHERE image_pk=2", [])
            .unwrap();
        assert!(!list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0].pinned);
        db.execute("UPDATE images SET trashed=1 WHERE image_pk IN (1,2)", [])
            .unwrap();
        for pk in 10..110 {
            let id = format!("discarded-{pk}");
            db.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id,trashed) VALUES(?1,?2,?2,?2,'png',64,40,100,0,0,1,1)",params![pk,id]).unwrap();
            compact::upsert_auto_tag(
                &db,
                &id,
                "general",
                "blue_sky",
                Some("蓝天"),
                0.99,
                WD_TAGGER_MODEL_NAME,
                0,
            )
            .unwrap();
        }
        // Put the surviving image after more than a full pool of recycled rows.
        let survivor = root.join("survivor.png").to_string_lossy().to_string();
        fs::copy(&ids[2], &survivor).unwrap();
        db.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) SELECT 200,?1,?1,'survivor.png',ext,width,height,file_size,modified_at,imported_at,folder_id FROM images WHERE image_pk=3",[&survivor]).unwrap();
        compact::upsert_auto_tag(
            &db,
            &survivor,
            "general",
            "blue_sky",
            Some("蓝天"),
            0.995,
            WD_TAGGER_MODEL_NAME,
            0,
        )
        .unwrap();
        assert_eq!(
            list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0]
                .image
                .as_ref()
                .unwrap()
                .id,
            survivor
        );
        db.execute("UPDATE images SET trashed=0 WHERE image_pk=2", [])
            .unwrap();
        assert!(list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0].pinned);
        assert!(ids.iter().all(|id| Path::new(id).is_file()));
        let remapped = root.join("relocated.png").to_string_lossy().to_string();
        fs::rename(&ids[1], &remapped).unwrap();
        db.execute(
            "UPDATE images SET id=?1,path=?1 WHERE image_pk=2",
            [&remapped],
        )
        .unwrap();
        remap_choices(
            &db,
            &[BackupPathMapping {
                from: ids[1].clone(),
                to: remapped.clone(),
            }],
        )
        .unwrap();
        assert!(list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0].pinned);
        // A recycled integer key must never pin an unrelated replacement image.
        db.execute("UPDATE images SET id='replacement-id' WHERE image_pk=2", [])
            .unwrap();
        assert!(!list_tag_covers(vec!["blue_sky".into()], &state).unwrap()[0].pinned);
        drop(db);
        drop(state);
        assert!(root.starts_with(&base));
        fs::remove_dir_all(root).unwrap();
    }
}
