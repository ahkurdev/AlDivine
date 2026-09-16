-- chat client: submits input and renders incoming messages.
-- Runs in the sandboxed Lua runtime.

Aldivine.Events.on('chat:client:start', function(payload)
    Aldivine.UI.show('chat')
end)

Aldivine.Events.on('chat:client:submit', function(payload)
    if type(payload) == 'table' then
        Aldivine.Events.emit('chat:server:submit', payload)
    end
end)

Aldivine.Events.on('chat:client:message', function(payload)
    if type(payload) == 'table' then
        Aldivine.UI.append('chat', payload.sender, payload.text)
    end
end)
