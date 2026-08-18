import type { ReactNode } from "react"

export function SettingsMenuSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-xl bg-white/[0.045] p-1 ring-1 ring-inset ring-white/10">
      <div className="flex items-center gap-2 px-2 pb-1 pt-0.5">
        <h3 className="shrink-0 text-[10px] font-bold tracking-[0.12em] text-white/45">{title}</h3>
        <span className="h-px flex-1 bg-white/10" />
      </div>
      <div className="space-y-0.5">{children}</div>
    </section>
  )
}
