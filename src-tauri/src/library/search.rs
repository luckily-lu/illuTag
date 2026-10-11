use super::*;

pub fn search_gallery_image_ids_by_natural_language(
    query: String,
    candidate_image_ids: Option<Vec<String>>,
    state: &AppState,
) -> Result<Vec<String>, String> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return Ok(Vec::new());
    }

    ensure_clip_vector_cache_loaded(state)?;
    let text_embedding = run_chinese_clip_text_embedding_via_service(trimmed_query, state)?;
    if text_embedding.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_filter = candidate_image_ids.map(|list| {
        list.into_iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>()
    });
    if matches!(candidate_filter, Some(ref set) if set.is_empty()) {
        return Ok(Vec::new());
    }

    let cache_guard = state
        .clip_vector_cache
        .lock()
        .map_err(|_| "Clip vector cache state is locked".to_string())?;
    let cache = cache_guard
        .as_ref()
        .ok_or_else(|| "Clip vector cache not loaded".to_string())?;
    if cache.model_id != CHINESE_CLIP_MODEL_ID || cache.model_version != CHINESE_CLIP_MODEL_VERSION
    {
        return Ok(Vec::new());
    }
    if cache.dimension != text_embedding.len() {
        return Ok(Vec::new());
    }

    let top_k = NATURAL_LANGUAGE_SEARCH_DEFAULT_TOP_K.max(1);
    let mut heap = BinaryHeap::<NaturalLanguageSearchHeapEntry>::new();
    for (image_id, vector) in &cache.vectors {
        if let Some(filter) = &candidate_filter {
            if !filter.contains(image_id) {
                continue;
            }
        }
        if vector.len() != text_embedding.len() {
            continue;
        }
        let score = dot_product(&text_embedding, vector);
        if !score.is_finite() {
            continue;
        }
        heap.push(NaturalLanguageSearchHeapEntry {
            image_id: image_id.clone(),
            score,
        });
        if heap.len() > top_k {
            let _ = heap.pop();
        }
    }

    let mut ranked = Vec::<NaturalLanguageSearchHeapEntry>::with_capacity(heap.len());
    while let Some(entry) = heap.pop() {
        ranked.push(entry);
    }
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.image_id.cmp(&b.image_id))
    });
    Ok(ranked.into_iter().map(|entry| entry.image_id).collect())
}
