-- commands client: dispatches user input as commands.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('commands:client:run', function(payload)
    if type(payload) == 'table' and type(payload.command) == 'string' then
        Aldivine.Events.emit('commands:server:execute', payload)
    end
end)
