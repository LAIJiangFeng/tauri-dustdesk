import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react"

/** Marks popups (context menus, settings panels) that should suspend box keyboard shortcuts. */
export const desktopPopupAttribute = "data-dustdesk-popup"
/** Marks one item button of a desktop box. */
export const desktopItemAttribute = "data-desktop-box-item"
/** Marks the item list container of a desktop box. */
export const desktopItemListAttribute = "data-desktop-box-list"

export interface DesktopItemSelectionShortcuts {
  onCopy: (paths: string[]) => void
  onCopyPath: (paths: string[]) => void
  onOpen: (paths: string[]) => void
  onDelete: (paths: string[]) => void
  onCut: (paths: string[]) => void
  onRename: (path: string) => void
  onProperties: (paths: string[]) => void
  onPaste: () => void
}

/**
 * Windows-desktop-like selection for the items of one box: click, Ctrl+click, Shift+click,
 * Ctrl+A, Esc, plus Ctrl+C / Ctrl+Shift+C / Ctrl+X / Enter / Delete / F2 / Alt+Enter shortcuts on
 * the selection and a Ctrl+V hint.
 * `paths` is the visible order; selected paths that disappear from it are dropped.
 */
export function useDesktopItemSelection(visiblePaths: string[], shortcuts: DesktopItemSelectionShortcuts) {
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set())
  const anchorRef = useRef<string | null>(null)
  // F2 renames the most recently clicked item; Shift+click keeps the anchor but moves this.
  const lastClickedRef = useRef<string | null>(null)
  // Set by selectOnly for a path a coming refresh is expected to list (a renamed item); it
  // survives pruning until it shows up.
  const pendingSelectRef = useRef<string | null>(null)
  // Callers rebuild the array every render; key on its content (Windows paths never contain "\n").
  const pathsKey = visiblePaths.join("\n")
  const paths = useMemo(() => (pathsKey ? pathsKey.split("\n") : []), [pathsKey])

  useEffect(() => {
    const present = new Set(paths)
    if (pendingSelectRef.current && present.has(pendingSelectRef.current)) pendingSelectRef.current = null
    const pending = pendingSelectRef.current
    const keep = (path: string) => present.has(path) || path === pending
    if (anchorRef.current && !keep(anchorRef.current)) anchorRef.current = null
    if (lastClickedRef.current && !keep(lastClickedRef.current)) lastClickedRef.current = null
    setSelected((current) => {
      if ([...current].every(keep)) return current
      return new Set([...current].filter(keep))
    })
  }, [paths])

  const selectedPaths = useMemo(() => paths.filter((path) => selected.has(path)), [paths, selected])

  const isSelected = useCallback((path: string) => selected.has(path), [selected])

  const clearSelection = useCallback(() => {
    anchorRef.current = null
    setSelected((current) => (current.size === 0 ? current : new Set()))
  }, [])

  const selectAll = useCallback(() => {
    setSelected(new Set(paths))
  }, [paths])

  /** Selects exactly `path`, kept even if the refresh that lists it has not arrived yet. */
  const selectOnly = useCallback((path: string) => {
    anchorRef.current = path
    lastClickedRef.current = path
    pendingSelectRef.current = path
    setSelected(new Set([path]))
  }, [])

  const handleItemClick = useCallback(
    (path: string, event: ReactMouseEvent) => {
      const additive = event.ctrlKey || event.metaKey
      const anchor = anchorRef.current
      lastClickedRef.current = path
      pendingSelectRef.current = null
      if (event.shiftKey && anchor && paths.includes(anchor)) {
        const from = paths.indexOf(anchor)
        const to = paths.indexOf(path)
        const range = paths.slice(Math.min(from, to), Math.max(from, to) + 1)
        setSelected((current) => new Set(additive ? [...current, ...range] : range))
        return
      }
      anchorRef.current = path
      if (additive) {
        setSelected((current) => {
          const next = new Set(current)
          if (next.has(path)) next.delete(path)
          else next.add(path)
          return next
        })
        return
      }
      setSelected(new Set([path]))
    },
    [paths],
  )

  /** Right-click keeps an existing multi-selection; otherwise it selects only that item. */
  const handleItemContextMenu = useCallback(
    (path: string) => {
      lastClickedRef.current = path
      if (selected.has(path)) return selectedPaths
      anchorRef.current = path
      setSelected(new Set([path]))
      return [path]
    },
    [selected, selectedPaths],
  )

  const latestRef = useRef({ shortcuts, selectedPaths, selectAll, clearSelection })
  latestRef.current = { shortcuts, selectedPaths, selectAll, clearSelection }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || !shortcutsAvailable()) return
      const { shortcuts, selectedPaths, selectAll, clearSelection } = latestRef.current
      const ctrl = event.ctrlKey || event.metaKey
      const hasSelection = selectedPaths.length > 0

      if (event.altKey) {
        // Alt+Enter is the only Alt shortcut; other Alt chords stay with the WebView / system.
        if (event.key === "Enter" && !ctrl && !event.shiftKey && hasSelection && focusInItemList()) {
          event.preventDefault()
          if (!event.repeat) shortcuts.onProperties(selectedPaths)
        }
        return
      }

      if (ctrl && !event.shiftKey && event.code === "KeyA") {
        event.preventDefault()
        selectAll()
      } else if (event.key === "Escape" && !ctrl && hasSelection) {
        event.preventDefault()
        clearSelection()
      } else if (ctrl && event.code === "KeyC" && hasSelection) {
        // Without preventDefault the WebView would also copy the page text selection.
        event.preventDefault()
        if (event.shiftKey) shortcuts.onCopyPath(selectedPaths)
        else shortcuts.onCopy(selectedPaths)
      } else if (ctrl && !event.shiftKey && event.code === "KeyX" && hasSelection && focusInItemList()) {
        event.preventDefault()
        if (!event.repeat) shortcuts.onCut(selectedPaths)
      } else if (ctrl && !event.shiftKey && event.code === "KeyV" && focusInItemList()) {
        event.preventDefault()
        if (!event.repeat) shortcuts.onPaste()
      } else if (event.key === "F2" && !ctrl && !event.shiftKey && hasSelection && focusInItemList()) {
        event.preventDefault()
        const lastClicked = lastClickedRef.current
        if (!event.repeat) shortcuts.onRename(lastClicked && selectedPaths.includes(lastClicked) ? lastClicked : selectedPaths[0])
      } else if (event.key === "Enter" && !ctrl && !event.shiftKey && hasSelection && focusInItemList()) {
        // Also stops a focused item button from turning Enter into a click.
        event.preventDefault()
        if (!event.repeat) shortcuts.onOpen(selectedPaths)
      } else if (event.key === "Delete" && !ctrl && !event.shiftKey && hasSelection && focusInItemList()) {
        event.preventDefault()
        if (!event.repeat) shortcuts.onDelete(selectedPaths)
      }
    }

    globalThis.addEventListener("keydown", onKeyDown)
    return () => globalThis.removeEventListener("keydown", onKeyDown)
  }, [])

  return { selectedPaths, isSelected, clearSelection, selectAll, selectOnly, handleItemClick, handleItemContextMenu }
}

/**
 * Enter, Delete, F2, Alt+Enter, Ctrl+X and Ctrl+V belong to the item list only while focus is
 * nowhere in particular or inside the list; other focused controls (settings button, links...)
 * keep their own key handling.
 */
function focusInItemList() {
  const active = document.activeElement
  if (!active || active === document.body || active === document.documentElement) return true
  return Boolean(active.closest(`[${desktopItemAttribute}], [${desktopItemListAttribute}]`))
}

function shortcutsAvailable() {
  if (!document.hasFocus()) return false
  // A collapsed edge-hidden box keeps its item list mounted but invisible (and may keep focus);
  // items the user cannot see must not react to keys.
  const list = document.querySelector(`[${desktopItemListAttribute}]`)
  if (list instanceof HTMLElement && getComputedStyle(list).visibility === "hidden") return false
  const active = document.activeElement
  if (active instanceof HTMLElement) {
    if (active.isContentEditable) return false
    if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement || active instanceof HTMLSelectElement) return false
  }
  return !document.querySelector(`[${desktopPopupAttribute}], [role="dialog"], dialog[open]`)
}
