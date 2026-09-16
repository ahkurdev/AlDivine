-- loading client: manages load screen state and signals completion.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('loading:client:start', function(payload)
    Aldivine.UI.show('loading')
end)

Aldivine.Events.on('loading:client:progress', function(payload)
    if type(payload) == 'table' then
        Aldivine.Events.emit('loading:server:progress', payload)
    end
end)

Aldivine.Events.on('loading:client:dismiss', function(payload)
    Aldivine.UI.hide('loading')
    Aldivine.Events.emit('loading:server:done', payload or {})
end)
