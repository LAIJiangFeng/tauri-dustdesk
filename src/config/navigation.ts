import type { Icon } from "@phosphor-icons/react"
import {
  Archive,
  ClipboardText,
  GearSix,
  House,
  MagnifyingGlass,
  RocketLaunch,
} from "@phosphor-icons/react"
import type { AppPage } from "@/types"

export interface NavigationItem {
  page: AppPage
  path: string
  label: string
  hint: string
  icon: Icon
}

export interface PageMeta {
  title: string
}

export const navigationItems: NavigationItem[] = [
  { page: "home", path: "/", label: "主页", hint: "总览", icon: House },
  { page: "organizer", path: "/organizer", label: "收纳", hint: "桌面归档", icon: Archive },
  { page: "launcher", path: "/launcher", label: "启动", hint: "应用编队", icon: RocketLaunch },
  { page: "clipboard", path: "/clipboard", label: "剪贴板", hint: "历史卡片", icon: ClipboardText },
  { page: "search", path: "/search", label: "搜索", hint: "统一检索", icon: MagnifyingGlass },
  { page: "settings", path: "/settings", label: "设置", hint: "本地路径", icon: GearSix },
]

export const pageMeta: Record<AppPage, PageMeta> = {
  home: {
    title: "桌面工作台",
  },
  organizer: {
    title: "桌面收纳矩阵",
  },
  launcher: {
    title: "快捷启动编队",
  },
  clipboard: {
    title: "剪贴板卡片",
  },
  search: {
    title: "搜索设置",
  },
  settings: {
    title: "设置中心",
  },
}

export function pageFromPath(pathname: string): AppPage {
  return navigationItems.find((item) => item.path === pathname)?.page ?? "home"
}

export function pathForPage(page: AppPage) {
  return navigationItems.find((item) => item.page === page)?.path ?? "/"
}
