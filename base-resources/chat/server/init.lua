-- chat server: relays bounded messages to all clients.
-- Runs in the sandboxed Lua runtime. Bounds are enforced here so a
-- malicious client cannot fan out unbounded text.

local MAX_MESSAGE = 256
local MAX_SENDER = 32

local function clean(value, limit)
    if type(value) ~= 'string' then
        return nil
    end
    if #value == 0 or #value > limit then
        return nil
    end
    return value
end

Aldivine.Events.on('chat:server:submit', function(payload)
    if type(payload) ~= 'table' then
        return
    end
    local sender = clean(payload.sender, MAX_SENDER)
    local text = clean(payload.text, MAX_MESSAGE)
    if sender == nil or text == nil then
        return
    end
    Aldivine.Events.emit('chat:client:message', { sender = sender, text = text })
end)
