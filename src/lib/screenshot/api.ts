import { invoke } from "@tauri-apps/api/core"
import type { OcrResult, ScreenshotPinInfo, ScreenshotSession } from "@/types"

export const SCREENSHOT_START_EVENT = "dustdesk://screenshot-start"
export const SCREENSHOT_END_EVENT = "dustdesk://screenshot-end"

export function hasTauriRuntime() {
  return "__TAURI_INTERNALS__" in window
}

async function call<T>(command: string, args?: Parameters<typeof invoke>[1], headers?: Record<string, string>): Promise<T> {
  if (!hasTauriRuntime()) throw new Error("截图功能需要在 DustDesk 桌面程序中使用")
  return invoke<T>(command, args, headers ? { headers } : undefined)
}

export async function getScreenshotSession(monitorIndex: number) {
  if (!hasTauriRuntime()) return null
  return (await call<ScreenshotSession | null>("get_screenshot_session", { monitorIndex })) ?? null
}

/** Tells the backend this overlay painted its first frame for the session, so it may be moved on-screen. */
export async function screenshotOverlayReady(sessionId: string, monitorIndex: number) {
  if (!hasTauriRuntime()) return
  await call<void>("screenshot_overlay_ready", { sessionId, monitorIndex })
}

export function screenshotCopy(png: Uint8Array) {
  return call<void>("screenshot_copy", png)
}

export function screenshotSave(png: Uint8Array) {
  return call<string | null>("screenshot_save", png)
}

export function screenshotPin(png: Uint8Array, rect: { x: number; y: number; width: number; height: number }) {
  return call<string>("screenshot_pin", png, {
    "x-dustshot-rect": `${Math.round(rect.x)},${Math.round(rect.y)},${Math.round(rect.width)},${Math.round(rect.height)}`,
  })
}

export function screenshotOcr(png: Uint8Array) {
  return call<OcrResult>("screenshot_ocr", png)
}

export async function screenshotCancel() {
  if (!hasTauriRuntime()) return
  await call<void>("screenshot_cancel")
}

export async function getScreenshotPin(pinId: string) {
  if (!hasTauriRuntime()) return null
  return call<ScreenshotPinInfo>("get_screenshot_pin", { pinId })
}

export function screenshotPinCopy(pinId: string) {
  return call<void>("screenshot_pin_copy", { pinId })
}

export function screenshotPinSave(pinId: string) {
  return call<string | null>("screenshot_pin_save", { pinId })
}

export async function screenshotPinClose(pinId: string) {
  if (!hasTauriRuntime()) return
  await call<void>("screenshot_pin_close", { pinId })
}

/** Reads the RGB color of a physical pixel, or null when it is outside the image. */
export type PixelReader = (x: number, y: number) => [number, number, number] | null

export interface ScreenshotImage {
  bitmap: ImageBitmap
  /** Null when the image could only be loaded through the <img> fallback. */
  pixels: PixelReader | null
}

/**
 * Reads pixels straight from an uncompressed 24/32bpp BMP file. Colors must not come from a canvas
 * readback of the decoded bitmap: a full-frame getImageData stalls the WebView2 renderer for seconds.
 */
export function bmpPixelReader(bytes: Uint8Array): PixelReader | null {
  if (bytes.length < 54 || bytes[0] !== 0x42 || bytes[1] !== 0x4d) return null
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const dataOffset = view.getUint32(10, true)
  const width = view.getInt32(18, true)
  const rawHeight = view.getInt32(22, true)
  const bytesPerPixel = view.getUint16(28, true) / 8
  const compression = view.getUint32(30, true)
  const height = Math.abs(rawHeight)
  const stride = (width * bytesPerPixel + 3) & ~3
  if (width <= 0 || height === 0 || (bytesPerPixel !== 3 && bytesPerPixel !== 4) || compression !== 0) return null
  if (dataOffset + stride * height > bytes.length) return null
  return (x, y) => {
    if (x < 0 || y < 0 || x >= width || y >= height) return null
    const row = rawHeight < 0 ? y : height - 1 - y
    const offset = dataOffset + row * stride + x * bytesPerPixel
    return [bytes[offset + 2], bytes[offset + 1], bytes[offset]]
  }
}

/**
 * Loads a dustshot:// image as a decoded bitmap that can be drawn into canvases without tainting them.
 * fetch is tried first (needs the protocol's CORS header); <img crossOrigin> is the fallback.
 */
export async function loadScreenshotImage(url: string, onFetched?: () => void): Promise<ScreenshotImage> {
  try {
    const response = await fetch(url)
    if (!response.ok) throw new Error(`截图读取失败（${response.status}）`)
    const bytes = new Uint8Array(await response.arrayBuffer())
    onFetched?.()
    const bitmap = await createImageBitmap(new Blob([bytes], { type: response.headers.get("content-type") ?? "image/bmp" }))
    return { bitmap, pixels: bmpPixelReader(bytes) }
  } catch (fetchError) {
    try {
      const image = new Image()
      image.crossOrigin = "anonymous"
      image.decoding = "async"
      image.src = url
      await image.decode()
      return { bitmap: await createImageBitmap(image), pixels: null }
    } catch {
      throw fetchError instanceof Error ? fetchError : new Error(String(fetchError))
    }
  }
}

export function errorText(reason: unknown) {
  if (reason instanceof Error) return reason.message
  return String(reason)
}

/** Makes html/body/#root transparent for overlay-style windows; returns a restore function. */
export function applyTransparentDocument() {
  const elements = [document.documentElement, document.body, document.getElementById("root")]
  const previous = elements.map((element) => element?.style.background ?? "")
  for (const element of elements) {
    if (element) element.style.background = "transparent"
  }
  document.documentElement.classList.remove("boot-main-route")
  return () => {
    elements.forEach((element, index) => {
      if (element) element.style.background = previous[index]
    })
  }
}
