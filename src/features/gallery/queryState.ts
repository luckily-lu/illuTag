import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";

export class QueryGeneration {
  private value = 0;
  next() {
    return ++this.value;
  }
  current(token: number) {
    return token === this.value;
  }
  invalidate() {
    this.value++;
  }
}

function normalizedTags(tags: readonly string[]) {
  const values = tags.map((tag) => tag.trim()).filter(Boolean);
  values.sort();
  return [...new Set(values)];
}

// Keep in sync with split_search_tokens in tag_search/inference: whitespace,
// lowercase, sorted, de-duplicated tokens.
function normalizedEnglishQuery(query: string) {
  const values = query
    .split(/\s+/)
    .map((token) => token.trim().toLowerCase())
    .filter(Boolean);
  values.sort();
  return [...new Set(values)];
}

/**
 * Identity of the visible result range. It intentionally excludes pagination
 * and the random seed: changing those does not replace the selected result set.
 * Tag/file normalization mirrors the Rust query path so equivalent filters do
 * not clear a selection for a semantically identical query.
 */
export function semanticQueryKey(query: GalleryQuery) {
  return JSON.stringify({
    scope: query.scope,
    folderId: query.folderId,
    unclassifiedOnlyParentFolderId: query.unclassifiedOnlyParentFolderId,
    tags: normalizedTags(query.tags),
    englishQuery: normalizedEnglishQuery(query.englishQuery),
    fileName: query.fileName.trim().toLowerCase(),
    naturalLanguage: query.naturalLanguage.trim(),
  });
}
