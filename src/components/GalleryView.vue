<script setup lang="ts">
import LoadingOne from '@icon-park/vue-next/es/icons/LoadingOne'
import Search from '@icon-park/vue-next/es/icons/Search'
import CircleDoubleUp from '@icon-park/vue-next/es/icons/CircleDoubleUp'
import AppSwitch from '@icon-park/vue-next/es/icons/AppSwitch'
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import SegmentedMasonry from './SegmentedMasonry.vue'
import type { GalleryLayoutItem } from '../types/gallery'

type DragState = {
  imageId: string
  x: number
  y: number
}

type KnownAutoTagSuggestion = {
  tagEn: string
  tagZh?: string | null
  imageCount: number
  isUserCustom?: boolean
}

type GalleryBatchActionItem = {
  key:
    | 'copy-folder'
    | 'move-folder'
    | 'add-tags'
    | 'remove-from-folder'
    | 'assign-folder'
    | 'favorite'
    | 'favorite-remove'
    | 'restore-trash'
    | 'system-trash'
    | 'trash'
  label: string
}

type GalleryBrowseMode = 'default' | 'sidebar-disabled' | 'carousel'

const props = defineProps<{
  visibleImages: Array<{ id: string }>
  searchPanelStyle: Record<string, string>
  searchRevealMode: 'inline' | 'hidden' | 'floating'
  isSearchFocused: boolean
  searchZhInput: string
  searchZhSelected: KnownAutoTagSuggestion[]
  searchZhSuggestions: KnownAutoTagSuggestion[]
  searchZhOpen: boolean
  searchEnQuery: string
  searchFileNameQuery: string
  searchNaturalLanguageQuery: string
  searchMode: 'text' | 'image'
  externalImageSearchType: 'default' | 'atmosphere' | 'color'
  externalImageQueryUrl: string
  externalImageQueryPreviewUrl: string
  externalImageQueryLabel: string
  searchConfidenceMin: number
  searchConfidenceMax: number
  searchRunning: boolean
  searchError: string
  galleryBrowseMode: GalleryBrowseMode
  isLoading: boolean
  largeLibraryMode: boolean
  galleryLoadedImageCount: number
  galleryTotalImageCount: number
  canLoadMoreGalleryImages: boolean
  isGalleryPageLoading: boolean
  showUnclassifiedToggle: boolean
  isUnclassifiedOnly: boolean
  favoriteImageIds: string[]
  isBatchMode: boolean
  isBatchAllSelected: boolean
  batchSelectedImageIds: string[]
  batchActionLabels: GalleryBatchActionItem[]
  layoutItems: GalleryLayoutItem[]
  totalHeight: number
  contentWidth: number
  dragState: DragState | null
  handlers: Record<string, (...args: any[]) => any>
}>()

const gallerySectionEl = ref<HTMLElement | null>(null)
const searchPanelEl = ref<HTMLElement | null>(null)
const gallerySectionResizeObserver = ref<ResizeObserver | null>(null)
const searchPanelResizeObserver = ref<ResizeObserver | null>(null)
const searchHideTimer = ref<number | null>(null)
const lastPointerClient = ref<{ x: number; y: number } | null>(null)
const viewportSyncRaf = ref<number | null>(null)
const browseModeMenuOpen = ref(false)
const suppressOutsideClickOnce = ref(false)
const searchChipsEl = ref<HTMLElement | null>(null)
const searchChipsDragState = ref<{
  pointerId: number
  startX: number
  startScrollLeft: number
} | null>(null)
const searchBarMode = ref<'tags' | 'file' | 'image' | 'nl'>('tags')
const imageSearchDragDepth = ref(0)
const imageSearchDragActive = ref(false)
const internalImageSearchDragActive = ref(false)
const batchSweepState = ref<{
  pointerId: number
  startClientX: number
  startClientY: number
  moved: boolean
  lastPaintedImageId: string | null
} | null>(null)
const suppressBatchImageClickOnce = ref(false)

function clearSearchHideTimer() {
  if (searchHideTimer.value !== null) {
    window.clearTimeout(searchHideTimer.value)
    searchHideTimer.value = null
  }
}

function clearViewportSyncRaf() {
  if (viewportSyncRaf.value !== null) {
    window.cancelAnimationFrame(viewportSyncRaf.value)
    viewportSyncRaf.value = null
  }
}

function scheduleSearchViewportSync() {
  clearViewportSyncRaf()
  viewportSyncRaf.value = window.requestAnimationFrame(() => {
    viewportSyncRaf.value = null
    syncSearchViewportState()
  })
}

function syncSearchViewportState() {
  const section = gallerySectionEl.value
  const panel = searchPanelEl.value
  if (!section || !panel) return
  props.handlers.setSearchViewportState(
    section.scrollTop,
    section.clientHeight,
    panel.offsetHeight,
    panel.offsetTop,
  )
}

function toggleBrowseModeMenu() {
  browseModeMenuOpen.value = !browseModeMenuOpen.value
}

function closeBrowseModeMenu() {
  browseModeMenuOpen.value = false
}

function setBrowseMode(mode: GalleryBrowseMode) {
  props.handlers.setGalleryBrowseMode(mode)
  closeBrowseModeMenu()
}

function browseModeLabel(mode: GalleryBrowseMode) {
  if (mode === 'sidebar-disabled') return '禁用侧栏'
  if (mode === 'carousel') return '轮播模式'
  return '默认模式'
}

function onWindowPointerDown(event: PointerEvent) {
  if (!browseModeMenuOpen.value) return
  const target = event.target as HTMLElement | null
  if (target?.closest('.gallery-search__browse-mode-wrap')) return
  closeBrowseModeMenu()
}

onMounted(async () => {
  console.info(`[startup-prof] GalleryView mounted_ms=${performance.now().toFixed(1)}`)
  console.info(`[startup-prof] GalleryView first_setGalleryElement_before_ms=${performance.now().toFixed(1)}`)
  props.handlers.setGalleryElement(gallerySectionEl.value)
  console.info(`[startup-prof] GalleryView first_setGalleryElement_after_ms=${performance.now().toFixed(1)}`)
  if (gallerySectionEl.value) {
    props.handlers.onGalleryScroll(gallerySectionEl.value.scrollTop, gallerySectionEl.value.clientHeight)
  }
  syncSearchViewportState()
  if (gallerySectionEl.value && typeof ResizeObserver !== 'undefined') {
    gallerySectionResizeObserver.value = new ResizeObserver(() => {
      scheduleSearchViewportSync()
    })
    gallerySectionResizeObserver.value.observe(gallerySectionEl.value)
  }
  if (searchPanelEl.value && typeof ResizeObserver !== 'undefined') {
    searchPanelResizeObserver.value = new ResizeObserver(() => {
      scheduleSearchViewportSync()
    })
    searchPanelResizeObserver.value.observe(searchPanelEl.value)
  }
  window.addEventListener('resize', scheduleSearchViewportSync, { passive: true })
  window.addEventListener('pointermove', trackPointerPosition, { passive: true })
  window.addEventListener('pointerdown', onWindowPointerDown, { passive: true })
  await nextTick()
  const firstScreenImgNodes = gallerySectionEl.value?.querySelectorAll('.masonry__item img').length ?? 0
  console.info(
    `[startup-prof] GalleryView first_screen_img_nodes=${firstScreenImgNodes}`,
  )
})

onBeforeUnmount(() => {
  clearSearchHideTimer()
  clearViewportSyncRaf()
  finishSearchChipsDrag()
  gallerySectionResizeObserver.value?.disconnect()
  gallerySectionResizeObserver.value = null
  searchPanelResizeObserver.value?.disconnect()
  searchPanelResizeObserver.value = null
  window.removeEventListener('resize', scheduleSearchViewportSync)
  window.removeEventListener('pointermove', trackPointerPosition)
  window.removeEventListener('pointerdown', onWindowPointerDown)
  props.handlers.setGalleryElement(null)
  props.handlers.setSearchPointerInside(false)
  props.handlers.setSearchFocus(false)
  clearBatchSweep()
})

function trackPointerPosition(event: PointerEvent) {
  lastPointerClient.value = { x: event.clientX, y: event.clientY }
}

function isPointerInSearchSafeArea() {
  const pointer = lastPointerClient.value
  if (!pointer) return false
  const searchPanel = searchPanelEl.value
  if (searchPanel) {
    const rect = searchPanel.getBoundingClientRect()
    const inHorizontalBridge = pointer.x >= rect.left - 12 && pointer.x <= rect.right + 12
    const inVerticalBridge = pointer.y >= 0 && pointer.y <= rect.bottom
    if (inHorizontalBridge && inVerticalBridge) return true
  }
  if (pointer.x < 0 || pointer.y < 0 || pointer.x > window.innerWidth || pointer.y > window.innerHeight) {
    return true
  }
  const element = document.elementFromPoint(pointer.x, pointer.y) as HTMLElement | null
  if (!element) return false
  if (element.closest('.gallery-search')) return true
  if (element.closest('.gallery-search-hotspot')) return true
  if (element.closest('.app-titlebar')) return true
  if (element.closest('.app-titlebar-hotspot')) return true
  return false
}

function onSearchFocusIn() {
  suppressOutsideClickOnce.value = false
  props.handlers.setSearchFocus(true)
}

function onSearchFocusOut(event: FocusEvent) {
  const current = event.currentTarget as HTMLElement | null
  const next = event.relatedTarget as Node | null
  if (current && next && current.contains(next)) return
  props.handlers.setSearchFocus(false)
}

function onSearchPointerEnter() {
  suppressOutsideClickOnce.value = false
  clearSearchHideTimer()
  props.handlers.setSearchPointerInside(true)
}

function onSearchPointerLeave() {
  props.handlers.setSearchPointerInside(false)
}

function setSearchBarMode(mode: 'tags' | 'file' | 'image' | 'nl') {
  searchBarMode.value = mode
  props.handlers.setSearchMode(mode === 'image' ? 'image' : 'text')
}

function onGalleryScrollEvent(event: Event) {
  const element = event.target as HTMLElement
  props.handlers.onGalleryScroll(element.scrollTop, element.clientHeight)
  syncSearchViewportState()
}

function onSearchHotspotEnter() {
  clearSearchHideTimer()
  props.handlers.triggerSearchRevealByHotspot()
}

function onSearchHotspotDragHover(event: DragEvent) {
  const dataTransfer = event.dataTransfer
  if (!dataTransfer || !hasImagePayload(dataTransfer)) return
  clearSearchHideTimer()
  props.handlers.triggerSearchRevealByHotspot()
  event.preventDefault()
}

function isTargetInsideSearch(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest('.gallery-search'))
}

function dismissSearchFocus() {
  const active = document.activeElement as HTMLElement | null
  if (active && searchPanelEl.value?.contains(active)) {
    active.blur()
  }
  props.handlers.setSearchFocus(false)
  props.handlers.setSearchPointerInside(false)
}

function onGalleryPointerDownCapture(event: PointerEvent) {
  if (props.isSearchFocused && !isTargetInsideSearch(event.target)) {
    dismissSearchFocus()
    suppressOutsideClickOnce.value = true
    event.preventDefault()
    event.stopPropagation()
    return
  }
  startBatchSweep(event)
}

function clearBatchSweep() {
  const section = gallerySectionEl.value
  const state = batchSweepState.value
  if (section && state && section.hasPointerCapture(state.pointerId)) {
    section.releasePointerCapture(state.pointerId)
  }
  batchSweepState.value = null
}

function isTargetInsideBatchActions(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest('.gallery-batch-actions'))
}

function imageIdFromPoint(clientX: number, clientY: number) {
  const node = document.elementFromPoint(clientX, clientY) as HTMLElement | null
  const item = node?.closest('.masonry__item') as HTMLElement | null
  if (!item) return null
  const imageId = item.dataset.galleryImageId
  return imageId && imageId.length > 0 ? imageId : null
}

function toggleBatchSelectionAtPoint(clientX: number, clientY: number) {
  const imageId = imageIdFromPoint(clientX, clientY)
  if (!imageId) return false
  props.handlers.toggleGalleryBatchImageSelection(imageId)
  return true
}

function paintBatchSelectionAtPoint(clientX: number, clientY: number) {
  const state = batchSweepState.value
  if (!state) return
  const imageId = imageIdFromPoint(clientX, clientY)
  if (!imageId || state.lastPaintedImageId === imageId) return
  state.lastPaintedImageId = imageId
  props.handlers.appendGalleryBatchImageSelection([imageId])
}

function startBatchSweep(event: PointerEvent) {
  if (!props.isBatchMode || event.button !== 0) return
  if (isTargetInsideSearch(event.target)) return
  if (isTargetInsideBatchActions(event.target)) return
  const section = gallerySectionEl.value
  if (!section) return
  batchSweepState.value = {
    pointerId: event.pointerId,
    startClientX: event.clientX,
    startClientY: event.clientY,
    moved: false,
    lastPaintedImageId: null,
  }
  section.setPointerCapture(event.pointerId)
}

function onGalleryPointerMoveCapture(event: PointerEvent) {
  const state = batchSweepState.value
  if (!state || state.pointerId !== event.pointerId) return
  if (!state.moved) {
    const dx = Math.abs(event.clientX - state.startClientX)
    const dy = Math.abs(event.clientY - state.startClientY)
    state.moved = dx > 4 || dy > 4
  }
  if (state.moved) {
    paintBatchSelectionAtPoint(event.clientX, event.clientY)
  }
  event.preventDefault()
  event.stopPropagation()
}

function onGalleryPointerUpCapture(event: PointerEvent) {
  const state = batchSweepState.value
  if (!state || state.pointerId !== event.pointerId) return
  if (state.moved) {
    suppressBatchImageClickOnce.value = true
  } else {
    suppressBatchImageClickOnce.value = toggleBatchSelectionAtPoint(event.clientX, event.clientY)
  }
  clearBatchSweep()
}

function onGalleryPointerCancelCapture(event: PointerEvent) {
  const state = batchSweepState.value
  if (!state || state.pointerId !== event.pointerId) return
  clearBatchSweep()
}

function onGalleryClickCapture(event: MouseEvent) {
  if (!suppressOutsideClickOnce.value) return
  if (isTargetInsideSearch(event.target)) {
    suppressOutsideClickOnce.value = false
    return
  }
  suppressOutsideClickOnce.value = false
  event.preventDefault()
  event.stopPropagation()
}

function onGalleryWheelCapture(event: WheelEvent) {
  if (!props.isSearchFocused) return
  if (isTargetInsideSearch(event.target)) return
  dismissSearchFocus()
  suppressOutsideClickOnce.value = false
}

function onMasonryImagePointerDown(item: GalleryLayoutItem, event: PointerEvent) {
  if (props.isBatchMode) return
  props.handlers.startImagePress(item, event)
}

function onMasonryImagePointerUp() {
  if (props.isBatchMode) return
  props.handlers.clearImagePress()
}

function onMasonryImageClick(item: GalleryLayoutItem) {
  if (props.isBatchMode) {
    if (suppressBatchImageClickOnce.value) {
      suppressBatchImageClickOnce.value = false
      return
    }
    props.handlers.toggleGalleryBatchImageSelection(item.id)
    return
  }
  props.handlers.openGalleryImageDetail(item)
}

function onMasonryImageContextMenu(item: GalleryLayoutItem, event: MouseEvent) {
  if (props.isBatchMode) {
    event.preventDefault()
    return
  }
  props.handlers.openGalleryImageMenu(item, event)
}

function onSearchPointerDown() {
  suppressOutsideClickOnce.value = false
}

function finishSearchChipsDrag() {
  const container = searchChipsEl.value
  const state = searchChipsDragState.value
  if (container && state && container.hasPointerCapture(state.pointerId)) {
    container.releasePointerCapture(state.pointerId)
  }
  if (container) {
    container.classList.remove('is-dragging')
  }
  searchChipsDragState.value = null
}

function onSearchChipsPointerDown(event: PointerEvent) {
  if (event.button !== 0) return
  const target = event.target as HTMLElement | null
  if (target?.closest('.gallery-search__chip-remove')) return
  const container = searchChipsEl.value
  if (!container) return
  searchChipsDragState.value = {
    pointerId: event.pointerId,
    startX: event.clientX,
    startScrollLeft: container.scrollLeft,
  }
  container.classList.add('is-dragging')
  container.setPointerCapture(event.pointerId)
  event.preventDefault()
}

function onSearchChipsPointerMove(event: PointerEvent) {
  const state = searchChipsDragState.value
  const container = searchChipsEl.value
  if (!state || !container || state.pointerId !== event.pointerId) return
  const deltaX = event.clientX - state.startX
  container.scrollLeft = state.startScrollLeft - deltaX
  event.preventDefault()
}

function onSearchChipsPointerUp(event: PointerEvent) {
  const state = searchChipsDragState.value
  if (!state || state.pointerId !== event.pointerId) return
  finishSearchChipsDrag()
}

function onSearchChipsPointerCancel(event: PointerEvent) {
  const state = searchChipsDragState.value
  if (!state || state.pointerId !== event.pointerId) return
  finishSearchChipsDrag()
}

function onSearchChipsLostPointerCapture() {
  if (!searchChipsDragState.value) return
  finishSearchChipsDrag()
}

async function pasteExternalImageAndSearch() {
  await props.handlers.pasteExternalImageSearchFromClipboard()
}

async function onImageSearchUrlPaste(event: ClipboardEvent) {
  if (typeof props.handlers.pasteExternalImageSearchFromPasteEvent !== 'function') return
  event.preventDefault()
  await props.handlers.pasteExternalImageSearchFromPasteEvent(event)
}

function hasImagePayload(dataTransfer: DataTransfer) {
  const fileList = Array.from(dataTransfer.files ?? [])
  if (fileList.some((file) => file.type.startsWith('image/'))) return true
  const items = Array.from(dataTransfer.items ?? [])
  if (items.some((item) => item.kind === 'file' && item.type.startsWith('image/'))) return true
  return items.some((item) => item.kind === 'file') || Array.from(dataTransfer.types ?? []).includes('Files')
}

function clearImageSearchDragState() {
  imageSearchDragDepth.value = 0
  imageSearchDragActive.value = false
}

function syncInternalImageSearchDragState() {
  const state = props.dragState
  if (!state) {
    internalImageSearchDragActive.value = false
    return
  }
  const element = document.elementFromPoint(state.x, state.y) as HTMLElement | null
  internalImageSearchDragActive.value = Boolean(element?.closest('.gallery-search__lens-drop'))
}

function onImageSearchDragEnter(event: DragEvent) {
  const dataTransfer = event.dataTransfer
  if (!dataTransfer || !hasImagePayload(dataTransfer)) return
  imageSearchDragDepth.value += 1
  imageSearchDragActive.value = true
  event.preventDefault()
  event.stopPropagation()
}

function onImageSearchDragOver(event: DragEvent) {
  const dataTransfer = event.dataTransfer
  if (!dataTransfer || !hasImagePayload(dataTransfer)) return
  imageSearchDragActive.value = true
  event.preventDefault()
  event.stopPropagation()
}

function onImageSearchDragLeave(event: DragEvent) {
  if (!imageSearchDragActive.value) return
  imageSearchDragDepth.value = Math.max(0, imageSearchDragDepth.value - 1)
  if (imageSearchDragDepth.value === 0) {
    imageSearchDragActive.value = false
  }
  event.preventDefault()
  event.stopPropagation()
}

async function onImageSearchDrop(event: DragEvent) {
  const dataTransfer = event.dataTransfer
  clearImageSearchDragState()
  if (!dataTransfer) return
  event.preventDefault()
  event.stopPropagation()
  const files = Array.from(dataTransfer.files ?? [])
  const imageFile = files.find((file) => file.type.startsWith('image/')) ?? files[0]
  if (!imageFile) return
  if (typeof props.handlers.setExternalImageSearchFromFile !== 'function') return
  await props.handlers.setExternalImageSearchFromFile(imageFile)
}

watch(
  () => props.dragState,
  () => {
    syncInternalImageSearchDragState()
  },
  { immediate: true, deep: true },
)

watch(
  () => props.isBatchMode,
  (enabled) => {
    if (enabled) return
    suppressBatchImageClickOnce.value = false
    clearBatchSweep()
  },
)

function canScrollContainer(container: HTMLElement, deltaY: number) {
  if (container.scrollHeight <= container.clientHeight + 1) return false
  if (deltaY < 0) return container.scrollTop > 0
  if (deltaY > 0) return container.scrollTop + container.clientHeight < container.scrollHeight - 1
  return false
}

function onSearchWheel(event: WheelEvent) {
  const target = event.target as HTMLElement | null
  if (!target) {
    event.preventDefault()
    event.stopPropagation()
    return
  }

  const scrollable = target.closest<HTMLElement>('.gallery-search__suggestions, .gallery-search__chips')
  if (scrollable) {
    if (!canScrollContainer(scrollable, event.deltaY)) {
      event.preventDefault()
    }
    event.stopPropagation()
    return
  }

  event.preventDefault()
  event.stopPropagation()
}
</script>

<template>
  <section
    ref="gallerySectionEl"
    class="gallery-page"
    @pointerdown.capture="onGalleryPointerDownCapture($event)"
    @pointermove.capture="onGalleryPointerMoveCapture($event)"
    @pointerup.capture="onGalleryPointerUpCapture($event)"
    @pointercancel.capture="onGalleryPointerCancelCapture($event)"
    @click.capture="onGalleryClickCapture($event)"
    @wheel.capture="onGalleryWheelCapture($event)"
    @wheel.passive="handlers.onGalleryWheel($event as WheelEvent)"
    @scroll.passive="onGalleryScrollEvent($event)"
  >
    <div
      ref="searchPanelEl"
      class="gallery-search gallery-search--bar"
      @focusin="onSearchFocusIn"
      @focusout="onSearchFocusOut"
      @pointerdown="onSearchPointerDown"
      @mouseenter="onSearchPointerEnter"
      @mouseleave="onSearchPointerLeave"
      @wheel="onSearchWheel"
    >
      <div class="gallery-search__bar">
        <div class="gallery-search__modes">
          <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': searchBarMode === 'tags' }" @click="setSearchBarMode('tags')">标签</button>
          <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': searchBarMode === 'file' }" @click="setSearchBarMode('file')">文件名</button>
          <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': searchBarMode === 'image' }" @click="setSearchBarMode('image')">以图</button>
          <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': searchBarMode === 'nl' }" @click="setSearchBarMode('nl')">自然语言</button>
        </div>
        <input
          v-if="searchBarMode === 'tags'"
          class="gallery-search__input"
          type="text"
          :value="searchZhInput"
          placeholder="输入中文关键词"
          autocomplete="off"
          @focus="handlers.openSearchZhSuggestionPanel()"
          @input="handlers.setSearchZhInput(($event.target as HTMLInputElement).value)"
          @blur="handlers.closeSearchZhSuggestionPanelDeferred()"
        />
        <input
          v-else-if="searchBarMode === 'file'"
          class="gallery-search__input"
          type="text"
          :value="searchFileNameQuery"
          placeholder="文件名关键词"
          autocomplete="off"
          @input="handlers.setSearchFileNameQuery(($event.target as HTMLInputElement).value)"
          @keydown.enter.prevent="handlers.executeGallerySearch()"
        />
        <input
          v-else-if="searchBarMode === 'nl'"
          class="gallery-search__input"
          type="text"
          :value="searchNaturalLanguageQuery"
          placeholder="例如：白发女孩在夜景中"
          autocomplete="off"
          @input="handlers.setSearchNaturalLanguageQuery(($event.target as HTMLInputElement).value)"
          @keydown.enter.prevent="handlers.executeGallerySearch()"
        />
        <div v-else class="gallery-search__image-compact">
          <div class="gallery-search__mode-tabs">
            <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': externalImageSearchType === 'default' }" @click="handlers.setExternalImageSearchType('default')">默认</button>
            <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': externalImageSearchType === 'atmosphere' }" @click="handlers.setExternalImageSearchType('atmosphere')">氛围</button>
            <button type="button" class="gallery-search__mode-tab" :class="{ 'is-active': externalImageSearchType === 'color' }" @click="handlers.setExternalImageSearchType('color')">色彩</button>
          </div>
          <div
            class="gallery-search__lens-drop gallery-search__lens-drop--bar"
            :class="{
              'is-filled': Boolean(externalImageQueryPreviewUrl),
              'is-drag-active': imageSearchDragActive || internalImageSearchDragActive,
            }"
            @contextmenu.prevent="pasteExternalImageAndSearch"
            @dragenter.prevent="onImageSearchDragEnter"
            @dragover.prevent="onImageSearchDragOver"
            @dragleave.prevent="onImageSearchDragLeave"
            @drop.prevent="onImageSearchDrop"
          >
            <img v-if="externalImageQueryPreviewUrl" class="gallery-search__lens-thumb" :src="externalImageQueryPreviewUrl" alt="" />
            <span v-else>拖入</span>
          </div>
          <button type="button" class="gallery-search__lens-link" @click="pasteExternalImageAndSearch">粘贴</button>
          <button type="button" class="gallery-search__lens-link" @click="handlers.selectExternalImageSearchFile()">上传</button>
          <input
            class="gallery-search__input"
            type="text"
            :value="externalImageQueryUrl"
            placeholder="图片链接"
            @input="handlers.setExternalImageQueryUrl(($event.target as HTMLInputElement).value)"
            @paste="onImageSearchUrlPaste"
            @keydown.enter.prevent="handlers.executeGallerySearch()"
          />
        </div>
        <button
          type="button"
          class="gallery-search__submit"
          :disabled="searchRunning"
          @click="handlers.executeGallerySearch()"
          :aria-label="searchRunning ? '搜索中' : '开始搜索'"
        >
          <LoadingOne
            v-if="searchRunning"
            class="gallery-search__submit-icon is-loading"
            theme="outline"
            :size="16"
            :stroke-width="3"
            stroke-linecap="round"
            stroke-linejoin="round"
            :fill="['currentColor']"
          />
          <Search
            v-else
            class="gallery-search__submit-icon"
            theme="outline"
            :size="16"
            :stroke-width="3"
            stroke-linecap="round"
            stroke-linejoin="round"
            :fill="['currentColor']"
          />
        </button>
        <button type="button" class="gallery-search__back-top" aria-label="返回顶部" @click="handlers.scrollGalleryToCurrentTop()">
          <CircleDoubleUp
            class="gallery-search__back-top-icon"
            theme="outline"
            :size="18"
            :stroke-width="3"
            stroke-linecap="round"
            stroke-linejoin="round"
            :fill="['currentColor']"
          />
        </button>
        <div class="gallery-search__browse-mode-wrap">
          <button
            type="button"
            class="gallery-search__browse-mode"
            :class="{ 'is-active': galleryBrowseMode !== 'default' }"
            :aria-label="`浏览模式：${browseModeLabel(galleryBrowseMode)}`"
            aria-haspopup="menu"
            :aria-expanded="browseModeMenuOpen"
            @click.stop="toggleBrowseModeMenu"
          >
            <AppSwitch
              class="gallery-search__browse-mode-icon"
              theme="outline"
              :size="18"
              :stroke-width="3"
              stroke-linecap="round"
              stroke-linejoin="round"
              :fill="['currentColor']"
            />
          </button>
          <div v-if="browseModeMenuOpen" class="gallery-search__browse-menu" role="menu" @click.stop>
            <button type="button" class="gallery-search__browse-menu-item" :class="{ 'is-active': galleryBrowseMode === 'default' }" role="menuitem" @click="setBrowseMode('default')">默认模式</button>
            <button type="button" class="gallery-search__browse-menu-item" :class="{ 'is-active': galleryBrowseMode === 'sidebar-disabled' }" role="menuitem" @click="setBrowseMode('sidebar-disabled')">禁用侧栏</button>
            <button type="button" class="gallery-search__browse-menu-item" :class="{ 'is-active': galleryBrowseMode === 'carousel' }" role="menuitem" @click="setBrowseMode('carousel')">轮播模式</button>
          </div>
        </div>
      </div>
      <div
        v-if="searchBarMode === 'tags' && searchZhSelected.length > 0"
        ref="searchChipsEl"
        class="gallery-search__chips gallery-search__chips--bar"
        :class="{ 'is-dragging': Boolean(searchChipsDragState) }"
        @pointerdown="onSearchChipsPointerDown"
        @pointermove="onSearchChipsPointerMove"
        @pointerup="onSearchChipsPointerUp"
        @pointercancel="onSearchChipsPointerCancel"
        @lostpointercapture="onSearchChipsLostPointerCapture"
      >
        <span
          v-for="tag in searchZhSelected"
          :key="tag.tagEn"
          class="gallery-search__chip"
          :class="{ 'is-user-custom': Boolean(tag.isUserCustom) }"
        >
          <span class="gallery-search__chip-text">{{ tag.tagZh || tag.tagEn }}</span>
          <button type="button" class="gallery-search__chip-remove" @click.stop="handlers.removeSearchZhSuggestion(tag.tagEn)">×</button>
        </span>
      </div>
      <div v-if="searchBarMode === 'tags' && searchZhOpen" class="gallery-search__suggestions">
        <button
          v-for="item in searchZhSuggestions"
          :key="item.tagEn"
          class="gallery-search__suggestion"
          :class="{ 'is-user-custom': Boolean(item.isUserCustom) }"
          type="button"
          @mousedown.prevent="handlers.selectSearchZhSuggestion(item)"
        >
          <span>{{ item.tagZh || item.tagEn }}</span>
          <small>{{ item.tagEn }} · {{ item.imageCount }}{{ item.isUserCustom ? ' · 自定义' : '' }}</small>
        </button>
      </div>
      <div v-if="searchError" class="gallery-search__status">{{ searchError }}</div>
    </div>

    <div v-if="showUnclassifiedToggle" class="gallery-unclassified-toggle-row">
      <button type="button" class="gallery-unclassified-toggle" @click="handlers.toggleActiveFolderUnclassifiedOnly()">
        {{ isUnclassifiedOnly ? '返回' : '有未归类的图片' }}
      </button>
    </div>

    <div v-if="visibleImages.length === 0" class="empty-panel">
      <h2>还没有图片</h2>
      <p>选择“所有”查看全部图片，或先在设置里添加本地图库文件夹。</p>
      <button class="primary-button" type="button" :disabled="isLoading" @click="handlers.openSettings()">
        去添加
      </button>
    </div>
    <SegmentedMasonry
      v-else
      :items="layoutItems"
      :favorite-image-ids="favoriteImageIds"
      :batch-mode="isBatchMode"
      :batch-selected-image-ids="batchSelectedImageIds"
      :total-height="totalHeight"
      :content-width="contentWidth"
      :active-drag-image-id="dragState?.imageId ?? null"
      @image-pointer-down="onMasonryImagePointerDown"
      @image-pointer-up="onMasonryImagePointerUp"
      @image-favorite-toggle="handlers.toggleGalleryImageFavorite"
      @image-click="onMasonryImageClick"
      @image-context-menu="onMasonryImageContextMenu"
    />

    <div v-if="largeLibraryMode" class="gallery-page-footer">
      <span>已显示 {{ galleryLoadedImageCount }} / {{ galleryTotalImageCount }}</span>
      <button
        v-if="galleryLoadedImageCount < galleryTotalImageCount"
        class="secondary-button"
        type="button"
        :disabled="!canLoadMoreGalleryImages || isGalleryPageLoading"
        @click="handlers.loadMoreGalleryImages()"
      >
        {{ isGalleryPageLoading ? '加载中…' : '加载更多' }}
      </button>
    </div>

    <div v-if="isBatchMode" class="gallery-batch-actions" @click.stop @pointerdown.stop>
      <span class="gallery-batch-actions__count">已选 {{ batchSelectedImageIds.length }}</span>
      <button type="button" class="gallery-batch-actions__item" @click="handlers.toggleSelectAllGalleryBatchImages()">
        {{ isBatchAllSelected ? '取消全选' : '全选' }}
      </button>
      <button
        v-for="action in batchActionLabels"
        :key="action.key"
        type="button"
        class="gallery-batch-actions__item"
        @click="handlers.onGalleryBatchAction(action.key)"
      >
        {{ action.label }}
      </button>
      <button
        type="button"
        class="gallery-batch-actions__cancel"
        aria-label="取消批量操作"
        @click="handlers.exitGalleryBatchMode()"
      >
        ×
      </button>
    </div>
  </section>
</template>
