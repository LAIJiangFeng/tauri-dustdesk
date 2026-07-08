import { Component, type ErrorInfo, type ReactNode } from "react"

interface AppErrorBoundaryState {
  error: Error | null
}

export class AppErrorBoundary extends Component<{ children: ReactNode }, AppErrorBoundaryState> {
  state: AppErrorBoundaryState = { error: null }

  static getDerivedStateFromError(error: Error): AppErrorBoundaryState {
    return { error }
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("DeskNest render failed", error, errorInfo)
  }

  render() {
    if (!this.state.error) {
      return this.props.children
    }

    return (
      <main className="grid h-screen w-screen place-items-center bg-[#080b0d] p-8 text-[#f5f0e7]">
        <section className="max-w-2xl rounded-[28px] border border-[rgba(239,106,118,0.35)] bg-[rgba(239,106,118,0.1)] p-8 shadow-[0_30px_90px_rgba(0,0,0,0.35)]">
          <div className="text-sm font-black uppercase tracking-[0.24em] text-[#ff9da6]">DeskNest 前端渲染异常</div>
          <h1 className="mt-4 text-3xl font-black tracking-[-0.06em] text-white">界面没有正常加载</h1>
          <p className="mt-4 text-sm leading-7 text-[#c8bdb0]">{this.state.error.message}</p>
          <button
            className="mt-6 h-11 rounded-2xl border border-[rgba(238,184,94,0.45)] bg-[#eeb85e] px-5 text-sm font-black text-[#17120c]"
            onClick={() => window.location.reload()}
          >
            重新加载
          </button>
        </section>
      </main>
    )
  }
}
