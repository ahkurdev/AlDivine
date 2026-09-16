const BASE = '/aegis'

async function get<T>(path: string): Promise<T> {
  const res = await fetch(`${BASE}${path}`)
  if (!res.ok) throw new Error(`${path} -> ${res.status}`)
  return (await res.json()) as T
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const text = await res.text()
    throw new Error(`${path} -> ${res.status}: ${text}`)
  }
  return (await res.json()) as T
}

export interface Health {
  status: string
  version: string
  uptime_s: number
}

export interface ServerStatus {
  name: string
  max_players: number
  uptime_s: number
  running_resources: number
  known_resources: number
  sessions: number
  framework_players_online: number
}

export interface ResourceInfo {
  name: string
  state: string
}

export interface SetupStatus {
  first_run: boolean
}

export const api = {
  health: () => get<Health>('/health'),
  status: () => get<ServerStatus>('/status'),
  resources: () => get<ResourceInfo[]>('/resources'),
  resourceAction: (name: string, action: string) => post<{ accepted: boolean }>('/resources/' + name + '/' + action, {}),
  setupStatus: () => get<SetupStatus>('/setup/status'),
  setupComplete: (hostname: string, max_players: number, bind: string) =>
    post<{ written: string }>('/setup/complete', { hostname, max_players, bind }),
}
