// Aldivine TypeScript API (native mode) — v1 surface sketch.
// Developers write TypeScript; it is compiled to JS for the runtime.

export type Priority = "low" | "normal" | "high" | "critical";
export type Json = string | number | boolean | null | Json[] | { [k: string]: Json };

export interface PlayerRecord {
  aldivineId: string;
  name: string;
  job?: string;
  dimension?: number;
}

export interface RpcResult<T = Json> {
  ok: boolean;
  data?: T;
  error?: string;
}

export namespace Aldivine {
  export namespace Events {
    export function on(event: string, handler: (data: Json) => void, priority?: Priority): void;
    export function emit(event: string, data: Json): void;
  }

  export namespace RPC {
    export function call<T = Json>(
      method: string,
      payload: Json,
      timeoutMs?: number,
    ): Promise<RpcResult<T>>;
  }

  export namespace Players {
    export function get(source: number): PlayerRecord | null;
  }

  export namespace Entities {
    export function spawn(authority: "server" | { player: number } | { resource: number }): string;
    export function destroy(id: string): boolean;
  }

  export namespace Resources {
    export function start(name: string): void;
    export function stop(name: string): void;
  }

  export namespace Permissions {
    export function check(principal: string, permission: string): boolean;
  }

  export namespace Database {
    export function query(sql: string, params: Json[]): Promise<Json[]>;
  }

  export namespace Telemetry {
    export function counter(name: string, by?: number): void;
    export function gauge(name: string, value: number): void;
  }
}
