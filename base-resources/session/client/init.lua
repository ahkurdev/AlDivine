-- session client: acknowledges session start.
-- Runs in the sandboxed Lua runtime.

Aldivine.Events.on('session:client:start', function(payload)
    Aldivine.Events.emit('session:server:hello', { ready = true })
end)
