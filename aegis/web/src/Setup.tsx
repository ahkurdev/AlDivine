import { useState } from 'react'
import { api } from './api'

export function Setup({ onDone }: { onDone: () => void }) {
  const [hostname, setHostname] = useState('Aldivine Server')
  const [maxPlayers, setMaxPlayers] = useState('64')
  const [bind, setBind] = useState('0.0.0.0:30120')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  async function submit() {
    setBusy(true)
    setError(null)
    try {
      const n = Number.parseInt(maxPlayers, 10)
      if (!Number.isFinite(n)) throw new Error('max players must be a number')
      const done = await api.setupComplete(hostname, n, bind)
      alert(`Wrote ${done.written}. Restart ald-server to apply it.`)
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'setup failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="rounded bg-surface p-6">
      <h2 className="text-xl font-semibold">Welcome to Aegis first-run setup</h2>
      <p className="mt-1 text-sm text-slate-400">No server configuration was found. Three answers create one.</p>
      <label className="mt-4 block text-sm">
        Server name
        <input
          className="mt-1 block w-full rounded border border-slate-700 bg-ink px-3 py-2"
          value={hostname}
          onChange={(e) => setHostname(e.target.value)}
        />
      </label>
      <label className="mt-3 block text-sm">
        Max players
        <input
          className="mt-1 block w-full rounded border border-slate-700 bg-ink px-3 py-2"
          value={maxPlayers}
          onChange={(e) => setMaxPlayers(e.target.value)}
        />
      </label>
      <label className="mt-3 block text-sm">
        Bind address
        <input
          className="mt-1 block w-full rounded border border-slate-700 bg-ink px-3 py-2"
          value={bind}
          onChange={(e) => setBind(e.target.value)}
        />
      </label>
      {error && <p className="mt-3 text-sm text-red-400">{error}</p>}
      <button
        disabled={busy}
        className="mt-4 rounded bg-sky px-4 py-2 text-sm font-semibold text-ink disabled:opacity-40"
        onClick={submit}
      >
        {busy ? 'Writing…' : 'Write server.cfg'}
      </button>
    </div>
  )
}
