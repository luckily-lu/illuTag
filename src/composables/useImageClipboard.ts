import { invoke } from '@tauri-apps/api/core'

export async function copyImageToSystemClipboard(imageId: string) {
  let backendError: unknown = null
  try {
    await invoke('copy_image_to_system_clipboard_command', { imageId })
    return
  } catch (error) {
    backendError = error
  }

  try {
    await copyImageToSystemClipboardByWebApi(imageId)
    return
  } catch (error) {
    const backendRaw = formatRawErrorNameMessage(backendError)
    const fallbackRaw = formatRawErrorNameMessage(error)
    throw new Error(
      [
        `后端系统剪贴板写入失败：${backendRaw}`,
        `Web Clipboard 回退失败：${fallbackRaw}`,
        `能力检测：${formatClipboardFeatureAvailability()}`,
      ].join('\n'),
    )
  }
}

async function copyImageToSystemClipboardByWebApi(imageId: string) {
  const payload = await invoke<{ bytes: number[]; mime_type?: string; mimeType?: string }>(
    'read_image_bytes_command',
    {
      imageId,
    },
  )
  const mimeType = payload.mime_type || payload.mimeType || 'image/png'
  const sourceBlob = new Blob([new Uint8Array(payload.bytes)], { type: mimeType })
  const features = clipboardFeatureAvailability()
  if (!features.hasClipboardWrite || !features.hasClipboardItem) {
    throw new Error(`Clipboard API 不可用：${formatClipboardFeatureAvailability(features)}`)
  }

  let convertError: unknown = null
  let pngBlob: Blob | null = null
  try {
    pngBlob = await convertImageBlobToPng(sourceBlob)
  } catch (error) {
    convertError = error
  }

  const data: Record<string, Blob> = {}
  if (pngBlob) {
    data['image/png'] = pngBlob
  } else {
    data[mimeType] = sourceBlob
  }

  try {
    await navigator.clipboard.write([new ClipboardItem(data)])
  } catch (error) {
    const rawWrite = formatRawErrorNameMessage(error)
    const rawConvert = convertError ? formatRawErrorNameMessage(convertError) : null
    throw new Error(
      [
        `系统剪贴板写入失败：${rawWrite}`,
        rawConvert ? `PNG 转换错误：${rawConvert}` : null,
      ]
        .filter(Boolean)
        .join('\n'),
    )
  }
}

async function convertImageBlobToPng(blob: Blob) {
  if (typeof createImageBitmap !== 'function') {
    return null
  }
  const bitmap = await createImageBitmap(blob)
  try {
    const canvas = document.createElement('canvas')
    canvas.width = bitmap.width
    canvas.height = bitmap.height
    const ctx = canvas.getContext('2d')
    if (!ctx) {
      throw new Error('Canvas 2D 上下文不可用')
    }
    ctx.drawImage(bitmap, 0, 0)
    return await new Promise<Blob | null>((resolve, reject) => {
      canvas.toBlob((png) => {
        if (png) resolve(png)
        else reject(new Error('canvas.toBlob 返回空结果'))
      }, 'image/png')
    })
  } finally {
    bitmap.close()
  }
}

function formatRawErrorNameMessage(error: unknown) {
  if (error instanceof Error) return `${error.name}: ${error.message}`
  const maybe = error as { name?: unknown; message?: unknown } | null
  if (maybe && typeof maybe === 'object') {
    const name = typeof maybe.name === 'string' ? maybe.name : 'UnknownError'
    const message = typeof maybe.message === 'string' ? maybe.message : String(error)
    return `${name}: ${message}`
  }
  return `UnknownError: ${String(error)}`
}

function clipboardFeatureAvailability() {
  const hasNavigator = typeof navigator !== 'undefined'
  const hasClipboard = hasNavigator && 'clipboard' in navigator && !!navigator.clipboard
  const hasClipboardWrite =
    hasClipboard && typeof (navigator.clipboard as Clipboard).write === 'function'
  const hasClipboardItem = typeof ClipboardItem !== 'undefined'
  const hasCreateImageBitmap = typeof createImageBitmap === 'function'
  return {
    hasNavigator,
    hasClipboard,
    hasClipboardWrite,
    hasClipboardItem,
    hasCreateImageBitmap,
  }
}

function formatClipboardFeatureAvailability(features = clipboardFeatureAvailability()) {
  return `navigator=${features.hasNavigator}, clipboard=${features.hasClipboard}, clipboard.write=${features.hasClipboardWrite}, ClipboardItem=${features.hasClipboardItem}, createImageBitmap=${features.hasCreateImageBitmap}`
}

export function buildClipboardCopyErrorText(error: unknown, scene: string) {
  return [
    `${scene}失败`,
    `原始错误：${formatRawErrorNameMessage(error)}`,
    `能力检测：${formatClipboardFeatureAvailability()}`,
  ].join('\n')
}
