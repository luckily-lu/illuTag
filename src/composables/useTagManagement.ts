import { computed, ref, watch } from 'vue'

type TagManagementFolder = {
  id: number
  name: string
  sortOrder: number
  tags: string[]
}

type TagManagementState = {
  folders: TagManagementFolder[]
  unclassifiedTags: string[]
}

export type TagDictionaryGroup = {
  id: string
  zh: string
}

export type TagDictionaryBrowserItem = {
  tagEn: string
  tagZh?: string | null
  group: string
  imageCount: number
  sortIndex: number
}

type UseTagManagementOptions = {
  formatError: (error: unknown) => string
  setErrorText: (value: string) => void
}

const DICT_PAGE = 200

export function useTagManagement(options: UseTagManagementOptions) {
  const tagManagerOpen = ref(false)
  const tagManagerTab = ref<'custom' | 'dict'>('custom')
  const isTagManagerLoading = ref(false)
  const tagManagerFolders = ref<TagManagementFolder[]>([])
  const activeTagManagerFolderId = ref<number | null>(null)
  const tagManagerUnclassifiedTags = ref<string[]>([])
  const newTagManagerFolderName = ref('')
  const newTagManagerTagText = ref('')

  const dictGroups = ref<TagDictionaryGroup[]>([])
  const dictTags = ref<TagDictionaryBrowserItem[]>([])
  const dictLoaded = ref(false)
  const dictGroupId = ref('all')
  const dictQuery = ref('')
  const dictOnlyUsed = ref(false)
  const dictSort = ref<'count' | 'index' | 'zh' | 'en'>('count')
  const dictVisibleLimit = ref(DICT_PAGE)

  function normalizeState(raw: Record<string, unknown>): TagManagementState {
    const foldersRaw = Array.isArray(raw.folders) ? raw.folders : []
    const unclassifiedRaw = Array.isArray(raw.unclassifiedTags ?? raw.unclassified_tags)
      ? (raw.unclassifiedTags ?? raw.unclassified_tags)
      : []
    const folders = foldersRaw
      .filter((item): item is Record<string, unknown> => typeof item === 'object' && item !== null)
      .map((item) => ({
        id: Number(item.id ?? 0),
        name: String(item.name ?? ''),
        sortOrder: Number(item.sortOrder ?? item.sort_order ?? 0),
        tags: Array.isArray(item.tags)
          ? item.tags.map((value) => String(value ?? '').trim()).filter((value) => value.length > 0)
          : [],
      }))
      .filter((item) => Number.isFinite(item.id) && item.id > 0 && item.name.trim().length > 0)
    const unclassifiedTags: string[] = (unclassifiedRaw as unknown[])
      .map((item) => String(item ?? '').trim())
      .filter((value) => value.length > 0)
    return { folders, unclassifiedTags }
  }

  function applyState(next: TagManagementState) {
    tagManagerFolders.value = next.folders
    tagManagerUnclassifiedTags.value = next.unclassifiedTags
    if (
      activeTagManagerFolderId.value !== null &&
      !next.folders.some((folder) => folder.id === activeTagManagerFolderId.value)
    ) {
      activeTagManagerFolderId.value = null
    }
    if (activeTagManagerFolderId.value === null && next.folders.length > 0) {
      activeTagManagerFolderId.value = next.folders[0].id
    }
  }

  async function reloadTagManagementState() {
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('list_tag_management_state_command')
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function loadDictionaryBrowser(force = false) {
    if (dictLoaded.value && !force) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<{
        groups?: TagDictionaryGroup[]
        tags?: Array<{
          tagEn?: string
          tag_en?: string
          tagZh?: string | null
          tag_zh?: string | null
          group?: string
          imageCount?: number
          image_count?: number
          sortIndex?: number
          sort_index?: number
        }>
      }>('list_tag_dictionary_browser_command')
      dictGroups.value = Array.isArray(raw.groups) ? raw.groups : []
      dictTags.value = (raw.tags ?? []).map((item) => ({
        tagEn: String(item.tagEn ?? item.tag_en ?? ''),
        tagZh: item.tagZh ?? item.tag_zh ?? null,
        group: String(item.group ?? 'other'),
        imageCount: Number(item.imageCount ?? item.image_count ?? 0) || 0,
        sortIndex: Number(item.sortIndex ?? item.sort_index ?? Number.MAX_SAFE_INTEGER) || Number.MAX_SAFE_INTEGER,
      })).filter((item) => item.tagEn.length > 0)
      dictLoaded.value = true
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function openTagManager(tab: 'custom' | 'dict' = 'custom') {
    tagManagerOpen.value = true
    tagManagerTab.value = tab
    if (tab === 'dict') await loadDictionaryBrowser()
    else await reloadTagManagementState()
  }

  function closeTagManager() {
    tagManagerOpen.value = false
    tagManagerTab.value = 'custom'
  }

  async function showTagManagerTab(tab: 'custom' | 'dict') {
    tagManagerTab.value = tab
    if (tab === 'dict') await loadDictionaryBrowser()
  }

  async function createTagManagerFolder() {
    const name = newTagManagerFolderName.value.trim()
    if (!name) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('create_user_tag_folder_command', { name })
      newTagManagerFolderName.value = ''
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function createTagManagerTag() {
    const tagText = newTagManagerTagText.value.trim()
    if (!tagText) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('create_user_custom_tag_command', { tagText })
      newTagManagerTagText.value = ''
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function deleteTagManagerTag(tagText: string) {
    const normalized = tagText.trim()
    if (!normalized) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('delete_user_custom_tag_command', { tagText: normalized })
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function assignTagToFolder(tagText: string, folderId: number | null = activeTagManagerFolderId.value) {
    if (!folderId) return
    const normalized = tagText.trim()
    if (!normalized) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('assign_user_tag_to_folder_command', {
        folderId,
        tagText: normalized,
      })
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function unassignTag(tagText: string) {
    const normalized = tagText.trim()
    if (!normalized) return
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('unassign_user_tag_from_folder_command', {
        tagText: normalized,
      })
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  async function deleteTagManagerFolder(folderId: number) {
    try {
      isTagManagerLoading.value = true
      const { invoke } = await import('@tauri-apps/api/core')
      const raw = await invoke<Record<string, unknown>>('delete_user_tag_folder_command', {
        folderId,
      })
      applyState(normalizeState(raw))
    } catch (error) {
      options.setErrorText(options.formatError(error))
    } finally {
      isTagManagerLoading.value = false
    }
  }

  const dictGroupItems = computed(() => {
    const counts = new Map<string, number>()
    let total = 0
    for (const tag of dictTags.value) {
      if (dictOnlyUsed.value && tag.imageCount <= 0) continue
      counts.set(tag.group, (counts.get(tag.group) ?? 0) + 1)
      total += 1
    }
    return [
      { id: 'all', zh: '全部', count: total },
      ...dictGroups.value.map((group) => ({
        id: group.id,
        zh: group.zh,
        count: counts.get(group.id) ?? 0,
      })),
    ]
  })

  const filteredDictTags = computed(() => {
    const query = dictQuery.value.trim().toLowerCase()
    let rows = dictTags.value
    if (dictGroupId.value !== 'all') {
      rows = rows.filter((tag) => tag.group === dictGroupId.value)
    }
    if (dictOnlyUsed.value) {
      rows = rows.filter((tag) => tag.imageCount > 0)
    }
    if (query) {
      rows = rows.filter(
        (tag) =>
          tag.tagEn.toLowerCase().includes(query) ||
          (tag.tagZh ?? '').toLowerCase().includes(query),
      )
    }
    const sort = dictSort.value
    return rows.slice().sort((a, b) => {
      if (sort === 'count') return b.imageCount - a.imageCount || a.sortIndex - b.sortIndex
      if (sort === 'index') return a.sortIndex - b.sortIndex
      if (sort === 'zh') {
        return (a.tagZh || a.tagEn).localeCompare(b.tagZh || b.tagEn, 'zh-CN') || a.sortIndex - b.sortIndex
      }
      return a.tagEn.localeCompare(b.tagEn) || a.sortIndex - b.sortIndex
    })
  })

  const visibleDictTags = computed(() => filteredDictTags.value.slice(0, dictVisibleLimit.value))

  watch([dictGroupId, dictQuery, dictOnlyUsed, dictSort], () => {
    dictVisibleLimit.value = DICT_PAGE
  })

  function loadMoreDictTags() {
    dictVisibleLimit.value += DICT_PAGE
  }

  return {
    tagManagerOpen,
    tagManagerTab,
    isTagManagerLoading,
    tagManagerFolders,
    activeTagManagerFolderId,
    tagManagerUnclassifiedTags,
    newTagManagerFolderName,
    newTagManagerTagText,
    dictGroupId,
    dictQuery,
    dictOnlyUsed,
    dictSort,
    dictGroupItems,
    filteredDictTags,
    visibleDictTags,
    openTagManager,
    closeTagManager,
    showTagManagerTab,
    reloadTagManagementState,
    createTagManagerFolder,
    createTagManagerTag,
    deleteTagManagerTag,
    assignTagToFolder,
    unassignTag,
    deleteTagManagerFolder,
    loadMoreDictTags,
  }
}
