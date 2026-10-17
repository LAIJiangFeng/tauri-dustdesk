import { useEffect, useRef, useState } from "react"
import type { KeyboardEvent as ReactKeyboardEvent } from "react"
import { Input } from "@/components/ui/input"

const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"])
// 仅按 Ctrl 时保留输入框原生编辑行为（全选 / 复制 / 粘贴 / 剪切 / 撤销 / 重做）。
const CTRL_EDITING_CODES = new Set(["KeyA", "KeyC", "KeyV", "KeyX", "KeyZ", "KeyY"])

export function shortcutKeyFromEvent(event: KeyboardEvent) {
  if (MODIFIER_CODES.has(event.code)) return ""
  if (event.code.startsWith("Key")) return event.code.slice(3).toUpperCase()
  if (event.code.startsWith("Digit")) return event.code.slice(5)
  if (event.code === "Space") return "Space"
  return event.code || event.key
}

export function shortcutFromEvent(event: KeyboardEvent) {
  const modifiers: string[] = []
  if (event.ctrlKey) modifiers.push("Ctrl")
  if (event.altKey) modifiers.push("Alt")
  if (event.shiftKey) modifiers.push("Shift")
  if (event.metaKey) modifiers.push("Super")

  const key = shortcutKeyFromEvent(event)
  if (!key) return modifiers.join("+")
  return [...modifiers, key].join("+")
}

function hasCaptureModifier(event: KeyboardEvent) {
  return event.ctrlKey || event.altKey || event.metaKey
}

function isCtrlEditingKey(event: KeyboardEvent) {
  return event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey && CTRL_EDITING_CODES.has(event.code)
}

/** 草稿为 null 时跟随已保存的快捷键，设置加载或更新后自动同步。 */
export function useShortcutDraft(savedShortcut: string) {
  const [editedDraft, setEditedDraft] = useState<string | null>(null)
  return {
    draft: editedDraft ?? savedShortcut,
    setDraft: setEditedDraft,
    resetDraft: () => setEditedDraft(null),
  }
}

type ShortcutInputProps = {
  value: string
  placeholder: string
  isRecording: boolean
  /** 录制模式下缺少主键时的提示示例，例如 "Tab / V / Space"。 */
  mainKeyExample: string
  onDraftChange: (value: string) => void
  /** 输入框中按 Enter 保存当前草稿。 */
  onSubmit: () => void
  /** 录制模式识别到完整组合键后调用，由调用方退出录制并保存。 */
  onRecorded: (shortcut: string) => void
  onRecordingError: (message: string) => void
}

export function ShortcutInput({
  value,
  placeholder,
  isRecording,
  mainKeyExample,
  onDraftChange,
  onSubmit,
  onRecorded,
  onRecordingError,
}: ShortcutInputProps) {
  const inputRef = useRef<HTMLInputElement | null>(null)
  const [modifierPreview, setModifierPreview] = useState("")
  const [recognized, setRecognized] = useState("")

  useEffect(() => {
    if (isRecording) {
      setModifierPreview("")
      setRecognized("")
      inputRef.current?.focus()
      inputRef.current?.select()
    }
  }, [isRecording])

  const handleRecordingKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    event.preventDefault()
    event.stopPropagation()

    const nextShortcut = shortcutFromEvent(event.nativeEvent)
    if (!nextShortcut) return

    onDraftChange(nextShortcut)
    if (!shortcutKeyFromEvent(event.nativeEvent)) {
      onRecordingError(`继续按一个主键，例如 ${mainKeyExample}`)
      return
    }

    if (!nextShortcut.includes("+")) {
      onRecordingError("请按住 Ctrl / Alt / Shift / Win 中至少一个修饰键")
      return
    }

    onRecorded(nextShortcut)
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (isRecording) {
      handleRecordingKeyDown(event)
      return
    }

    const nativeEvent = event.nativeEvent
    if (nativeEvent.isComposing) return

    if (event.key === "Enter" && !hasCaptureModifier(nativeEvent) && !event.shiftKey) {
      event.preventDefault()
      setModifierPreview("")
      setRecognized("")
      onSubmit()
      return
    }

    if (!hasCaptureModifier(nativeEvent)) return

    if (MODIFIER_CODES.has(event.code)) {
      setModifierPreview(`${shortcutFromEvent(nativeEvent)}+…`)
      return
    }

    if (isCtrlEditingKey(nativeEvent)) return

    event.preventDefault()
    event.stopPropagation()
    const nextShortcut = shortcutFromEvent(nativeEvent)
    onDraftChange(nextShortcut)
    setRecognized(nextShortcut)
    setModifierPreview("")
  }

  const handleKeyUp = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (isRecording) return
    if (!hasCaptureModifier(event.nativeEvent)) {
      setModifierPreview("")
      return
    }
    if (MODIFIER_CODES.has(event.code)) {
      setModifierPreview(`${shortcutFromEvent(event.nativeEvent)}+…`)
    }
  }

  const hint = modifierPreview || (recognized ? `已识别 ${recognized}，按 Enter 或点击保存生效` : "")

  return (
    <div className="grid gap-1">
      <Input
        ref={inputRef}
        value={value}
        placeholder={placeholder}
        className="h-11 font-mono"
        onChange={(event) => {
          setRecognized("")
          onDraftChange(event.target.value)
        }}
        onKeyDown={handleKeyDown}
        onKeyUp={handleKeyUp}
        onBlur={() => {
          setModifierPreview("")
          setRecognized("")
        }}
      />
      <p className="text-xs leading-5 text-muted-foreground">点击输入框后直接按下组合键即可识别，也可手动输入</p>
      {hint ? <p className="text-xs leading-5 font-medium text-primary">{hint}</p> : null}
    </div>
  )
}
