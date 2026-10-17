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

/**
 * Loads a dustshot:// image as a decoded bitmap that can be drawn into canvases without tainting them.
 * fetch is tried first (needs the protocol's CORS header); <img crossOrigin> is the fallback.
 */
export async function loadScreenshotBitmap(url: string): Promise<ImageBitmap> {
  try {
    const response = await fetch(url)
    if (!response.ok) throw new Error(`截图读取失败（${response.status}）`)
    const blob = await response.blob()
    return await createImageBitmap(blob)
  } catch (fetchError) {
    try {
      const image = new Image()
      image.crossOrigin = "anonymous"
      image.decoding = "async"
      image.src = url
      await image.decode()
      return await createImageBitmap(image)
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
