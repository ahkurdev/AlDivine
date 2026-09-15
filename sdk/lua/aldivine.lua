-- Aldivine Lua API (native mode) — type hints reference.
-- Full API surface is versioned. This is the v1 surface sketch.

---@class Aldivine
Aldivine = {}

---@class Aldivine.Events
Aldivine.Events = {
  --- Register an event handler.
  ---@param event string Namespaced event name (e.g. "player:joined").
  ---@param handler function
  ---@param priority? "low"|"normal"|"high"|"critical"
  on = function(event, handler, priority) end,

  --- Emit an event locally or over the network.
  ---@param event string
  ---@param data table
  emit = function(event, data) end,
}

---@class Aldivine.RPC
Aldivine.RPC = {
  ---@param method string
  ---@param payload table
  ---@param timeout_ms? number
  ---@param cb function(result: table|nil, err: string|nil)
  call = function(method, payload, timeout_ms, cb) end,
}

---@class Aldivine.Players
Aldivine.Players = {
  ---@param source number Resource-local player handle.
  ---@return table|nil Native player record.
  get = function(source) end,
}

---@class Aldivine.Entities
Aldivine.Entities = {
  spawn = function(authority) end,
  destroy = function(id) end,
}

---@class Aldivine.Resources
Aldivine.Resources = {
  start = function(name) end,
  stop = function(name) end,
}

---@class Aldivine.Permissions
Aldivine.Permissions = {
  check = function(principal, permission) end,
}

---@class Aldivine.Database
Aldivine.Database = {
  query = function(sql, params) end,
}

---@class Aldivine.Network
Aldivine.Network = {}

---@class Aldivine.UI
Aldivine.UI = {}

---@class Aldivine.Inventory
Aldivine.Inventory = {}

---@class Aldivine.Jobs
Aldivine.Jobs = {}

---@class Aldivine.Economy
Aldivine.Economy = {}

---@class Aldivine.Telemetry
Aldivine.Telemetry = {}

return Aldivine
