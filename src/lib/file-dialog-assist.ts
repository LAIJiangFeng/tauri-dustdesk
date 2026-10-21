import { invoke } from "@tauri-apps/api/core"

export const fileDialogAssistChangedEvent = "dustdesk://file-dialog-assist-changed"

export type FileDialogKind = "open" | "save" | "folder"
export type FileDialogActionMode = "auto" | "locate"
export type FileDialogActionOutcome = "done" | "elevated"

export interface FileDialogInfo {
  id: string
  kind: FileDialogKind
}

export interface FileDialogShortcut {
  name: string
  path: string
}

export interface FileDialogAssistState {
  dialog: FileDialogInfo | null
  organizer_root: string
  shortcuts: FileDialogShortcut[]
}

export interface FileDialogEntry {
  name: string
  path: string
  parent: string
  is_dir: boolean
  extension: string
  modified_ms: number | null
  highlight: number[]
}

type RawRecord = Record<string, unknown>

function hasTauriInternals() {
  return "__TAURI_INTERNALS__" in window
}

function asRecord(value: unknown): RawRecord {
  return value && typeof value === "object" ? (value as RawRecord) : {}
}

function asString(value: unknown) {
  return typeof value === "string" ? value : ""
}

function asKind(value: unknown): FileDialogKind {
  return value === "save" || value === "folder" ? value : "open"
}

export function normalizeFileDialogInfo(value: unknown): FileDialogInfo | null {
  if (!value || typeof value !== "object") return null
  const raw = asRecord(value)
  return { id: asString(raw.id), kind: asKind(raw.kind) }
}

function normalizeEntry(value: unknown): FileDialogEntry {
  const raw = asRecord(value)
  const modified = Number(raw.modified_ms)
  return {
    name: asString(raw.name),
    path: asString(raw.path),
    parent: asString(raw.parent),
    is_dir: raw.is_dir === true,
    extension: asString(raw.extension),
    modified_ms: raw.modified_ms === null || !Number.isFinite(modified) ? null : modified,
    highlight: Array.isArray(raw.highlight) ? raw.highlight.map(Number).filter(Number.isInteger) : [],
  }
}

function normalizeEntries(value: unknown) {
  return Array.isArray(value) ? value.map(normalizeEntry).filter((entry) => entry.path) : []
}

export async function loadFileDialogAssistState(): Promise<FileDialogAssistState> {
  if (!hasTauriInternals()) return { dialog: null, organizer_root: "", shortcuts: [] }
  const raw = asRecord(await invoke<unknown>("file_dialog_assist_state"))
  const shortcuts = Array.isArray(raw.shortcuts)
    ? raw.shortcuts.map((item) => {
        const shortcut = asRecord(item)
        return { name: asString(shortcut.name), path: asString(shortcut.path) }
      })
    : []
  return {
    dialog: normalizeFileDialogInfo(raw.dialog),
    organizer_root: asString(raw.organizer_root),
    shortcuts: shortcuts.filter((shortcut) => shortcut.path),
  }
}

export async function searchFileDialogEntries(query: string, root: string | null) {
  if (!hasTauriInternals()) return []
  return normalizeEntries(await invoke<unknown>("file_dialog_assist_search", { query, root }))
}

export async function listFileDialogEntries(path: string | null) {
  if (!hasTauriInternals()) return []
  return normalizeEntries(await invoke<unknown>("file_dialog_assist_list", { path }))
}

export async function executeFileDialogEntry(path: string, mode: FileDialogActionMode): Promise<FileDialogActionOutcome> {
  if (!hasTauriInternals()) return "done"
  const outcome = await invoke<unknown>("file_dialog_assist_execute", { path, mode })
  return outcome === "elevated" ? "elevated" : "done"
}

export async function focusFileDialog() {
  if (!hasTauriInternals()) return
  await invoke("file_dialog_assist_focus_dialog")
}

const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"]

/** 「今天 14:36」「昨天 09:10」「周三 14:54」, then dates for anything older than a week. */
export function formatRelativeTime(timestamp: number | null, now = new Date()) {
  if (timestamp === null) return ""
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return ""
  const time = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`
  const startOfDay = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime()
  const days = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000)
  if (days === 0) return `今天 ${time}`
  if (days === 1) return `昨天 ${time}`
  if (days > 1 && days < 7) return `${weekdays[date.getDay()]} ${time}`
  if (date.getFullYear() === now.getFullYear()) return `${date.getMonth() + 1}月${date.getDate()}日`
  return `${date.getFullYear()}/${date.getMonth() + 1}/${date.getDate()}`
}
