import { useEffect, useState } from 'react'
import { api, type ResourceInfo, type ServerStatus } from './api'

function stateColor(state: string): string {
  if (state === 'Healthy') return 'text-green-400'
  if (state === 'Degraded') return 'text-amber-400'
  if (state === 'Failed' || state === 'Quarantined') return 'text-red-400'
  return 'text-slate-400'
}

export function Dashboard({ onDone }: { onDone: () => void }) {
  const [status, setStatus] = useState<ServerStatus | null>(null)
  const [resources, setResources] = useState<ResourceInfo[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  async function refresh() {
    try {
      setError(null)
      const [s, r] = await Promise.all([api.status(), api.resources()])
      setStatus(s)
      setResources(r)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'request failed')
    }
  }

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 5000)
    return () => clearInterval(t)
  }, [])

  async function act(name: string, action: string) {
    setBusy(`${action}:${name}`)
    try {
      await api.resourceAction(name, action)
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'action failed')
    } finally {
      setBusy(null)
    }
  }

  if (error && !status) {
    return (
      <div className="rounded bg-surface p-6">
        <h2 className="text-lg font-semibold text-red-400">Cannot reach the server</h2>
        <p className="mt-2 text-sm text-slate-400">{error}</p>
        <p className="mt-2 text-sm text-slate-400">Is ald-server running with the Aegis API on 127.0.0.1:40120?</p>
        <button className="mt-4 rounded bg-sky px-4 py-2 text-sm font-semibold text-ink" onClick={refresh}>
          Retry
        </button>
      </div>
    )
  }

  if (!status || !resources) {
    return <div className="rounded bg-surface p-6 text-slate-400">Loading server state…</div>
  }

  return (
    <div className="space-y-4">
      <div className="rounded bg-surface p-6">
        <div className="flex items-baseline justify-between">
          <h2 className="text-xl font-semibold text-sky">{status.name}</h2>
          <button className="text-sm text-slate-400 underline" onClick={onDone}>
            Refresh setup check
          </button>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-2 text-sm md:grid-cols-4">
          <div>Uptime: {status.uptime_s}s</div>
          <div>Sessions: {status.sessions}</div>
          <div>Players online: {status.framework_players_online}</div>
          <div>
            Resources: {status.running_resources}/{status.known_resources}
          </div>
        </div>
        {error && <p className="mt-2 text-sm text-amber-400">{error}</p>}
      </div>
      <div className="rounded bg-surface p-6">
        <h3 className="font-semibold">Resources</h3>
        {resources.length === 0 && <p className="mt-2 text-sm text-slate-400">No resources known yet.</p>}
        <ul className="mt-2 divide-y divide-slate-800">
          {resources.map((r) => (
            <li key={r.name} className="flex items-center justify-between py-2">
              <span>
                <span className="font-mono">{r.name}</span>{' '}
                <span className={`text-sm ${stateColor(r.state)}`}>{r.state}</span>
              </span>
              <span className="space-x-2 text-sm">
                {['start', 'restart', 'stop'].map((a) => (
                  <button
                    key={a}
                    disabled={busy !== null}
                    className="rounded border border-slate-700 px-2 py-1 disabled:opacity-40"
                    onClick={() => act(r.name, a)}
                  >
                    {busy === `${a}:${r.name}` ? '…' : a}
                  </button>
                ))}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
