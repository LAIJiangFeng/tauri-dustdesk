import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function truncate(value: string | undefined | null, maxLength: number) {
  const text = value?.trim() ?? ""
  if (text.length <= maxLength) return text
  return `${text.slice(0, Math.max(0, maxLength - 1)).trimEnd()}...`
}

export function displayPathName(path: string | undefined | null) {
  const value = path?.trim() ?? ""
  if (!value) return "未命名"
  const name = pathFileName(value)
  const dotIndex = name.lastIndexOf(".")
  if (dotIndex <= 0) return name || "未命名"
  return name.slice(0, dotIndex) || name
}

export function pathFileName(path: string | undefined | null) {
  const value = path?.trim() ?? ""
  if (!value) return ""
  const normalized = value.replace(/\//g, "\\")
  const parts = normalized.split("\\").filter(Boolean)
  return parts.at(-1) ?? value
}

export function extensionFromPath(path: string | undefined | null, fallback = "FILE") {
  const name = pathFileName(path)
  const dotIndex = name.lastIndexOf(".")
  const extension = dotIndex > 0 ? name.slice(dotIndex + 1) : fallback
  return (extension || fallback).toUpperCase()
}

export function formatCount(value: number) {
  return new Intl.NumberFormat("zh-CN").format(value)
}
