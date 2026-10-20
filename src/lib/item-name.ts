import { pathFileName } from "@/lib/utils"

const invalidNameCharacters = /[\\/:*?"<>|\u0000-\u001f\u007f]/
const reservedDeviceName = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i

/**
 * Splits an item into the part the user edits and the part kept on rename: files keep their
 * extension (matching the stem shown in the box), folders edit the whole last path segment.
 */
export function editableItemName(path: string, isDir: boolean | undefined) {
  const fileName = pathFileName(path)
  if (isDir) return { editable: fileName, extension: "" }
  const dotIndex = fileName.lastIndexOf(".")
  if (dotIndex <= 0) return { editable: fileName, extension: "" }
  return { editable: fileName.slice(0, dotIndex), extension: fileName.slice(dotIndex) }
}

/**
 * Instant client-side check of a full new file name; the backend repeats it authoritatively and
 * also rejects names already used in the folder. Returns the reason, or null when valid.
 */
export function itemNameError(name: string): string | null {
  const trimmed = name.trim()
  if (!trimmed) return "名称不能为空"
  if (trimmed === "." || trimmed === "..") return "名称不能是 . 或 .."
  if (invalidNameCharacters.test(trimmed)) return '名称不能包含 \\ / : * ? " < > | 或控制字符'
  if (trimmed.endsWith(".")) return "名称不能以点号结尾"
  const base = trimmed.split(".")[0].trimEnd()
  if (reservedDeviceName.test(base)) return `${base} 是 Windows 保留名称，不能使用`
  return null
}
