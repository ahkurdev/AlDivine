import { useEffect, useState } from 'react'
import { api } from './api'
import { Dashboard } from './Dashboard'
import { Setup } from './Setup'

export function App() {
  const [firstRun, setFirstRun] = useState<boolean | null>(null)
  const [failed, setFailed] = useState(false)

  async function check() {
    try {
      const s = await api.setupStatus()
      setFirstRun(s.first_run)
      setFailed(false)
    } catch {
      setFailed(true)
    }
  }

  useEffect(() => {
    check()
  }, [])

  return (
    <div className="mx-auto max-w-3xl space-y-4 p-6">
      <header>
        <h1 className="text-2xl font-bold text-sky-strong">Aegis</h1>
        <p className="text-sm text-slate-400">Aldivine server control — local only</p>
      </header>
      {failed && (
        <div className="rounded bg-surface p-6">
          <p className="text-red-400">Cannot reach the Aegis API on 127.0.0.1:40120.</p>
          <button className="mt-3 rounded bg-sky px-4 py-2 text-sm font-semibold text-ink" onClick={check}>
            Retry
          </button>
        </div>
      )}
      {!failed && firstRun === null && <div className="rounded bg-surface p-6 text-slate-400">Checking setup…</div>}
      {!failed && firstRun === true && <Setup onDone={check} />}
      {!failed && firstRun === false && <Dashboard onDone={check} />}
    </div>
  )
}
