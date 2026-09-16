-- help client: requests help topics and formats output.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('help:client:open', function(payload)
    Aldivine.Events.emit('help:server:request', {})
end)

Aldivine.Events.on('help:client:response', function(payload)
    if type(payload) == 'table' and type(payload.topics) == 'table' then
        for cmd, desc in pairs(payload.topics) do
            Aldivine.UI.append('chat', 'HELP', '/' .. cmd .. ': ' .. desc)
        end
    end
end)
