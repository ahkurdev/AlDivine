-- loading server: tracks client loading status and handshake completion.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('loading:server:progress', function(payload)
    if type(payload) == 'table' and type(payload.fraction) == 'number' then
        Aldivine.Events.emit('loading:server:client_status', payload)
    end
end)

Aldivine.Events.on('loading:server:done', function(payload)
    Aldivine.Events.emit('session:server:ready', payload)
end)
