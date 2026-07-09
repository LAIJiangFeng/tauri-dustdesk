import {
  Command,
  File,
  FileArchive,
  FileCode,
  FileImage,
  FileText,
  FolderOpen,
  type Icon,
} from "@phosphor-icons/react"
import { cn } from "@/lib/utils"

interface FileIconProps {
  name: string
  extension?: string
  isDir?: boolean
  iconDataUrl?: string
  className?: string
  imageClassName?: string
}

const imageExtensions = new Set(["PNG", "JPG", "JPEG", "WEBP", "SVG", "GIF", "ICO"])
const archiveExtensions = new Set(["ZIP", "RAR", "7Z", "TAR", "GZ"])
const codeExtensions = new Set(["TS", "TSX", "JS", "JSX", "RS", "CS", "JSON", "CSS", "HTML", "YAML"])
const textExtensions = new Set(["TXT", "MD", "DOC", "DOCX", "PDF", "XLS", "XLSX", "PPT", "PPTX"])

export function FileIcon({ name, extension, isDir, iconDataUrl, className, imageClassName }: FileIconProps) {
  if (iconDataUrl) {
    return (
      <span className={cn("flex size-11 shrink-0 items-center justify-center", className)}>
        <img draggable={false} src={iconDataUrl} alt={`${name} 图标`} className={cn("pointer-events-none h-[82%] w-[82%] select-none object-contain", imageClassName)} />
      </span>
    )
  }

  const Icon = iconForFile((extension || "FILE").toUpperCase(), Boolean(isDir))
  return (
    <span className={cn("flex size-11 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground", className)}>
      <Icon className="size-6" weight="duotone" />
    </span>
  )
}

function iconForFile(extension: string, isDir: boolean): Icon {
  if (isDir) return FolderOpen
  if (extension === "LNK") return Command
  if (imageExtensions.has(extension)) return FileImage
  if (archiveExtensions.has(extension)) return FileArchive
  if (codeExtensions.has(extension)) return FileCode
  if (textExtensions.has(extension)) return FileText
  return File
}
