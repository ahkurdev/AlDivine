-- commands server: command registration and dispatch.
-- Runs in sandboxed Lua runtime.

local command_handlers = {}

Aldivine.Events.on('commands:server:execute', function(payload)
    if type(payload) ~= 'table' then
        return
    end
    local cmd = payload.command
    local args = payload.args or {}
    if type(cmd) == 'string' and command_handlers[cmd] then
        command_handlers[cmd](payload.source, args)
    end
end)

Aldivine.Events.on('commands:server:register', function(payload)
    if type(payload) == 'table' and type(payload.name) == 'string' then
        command_handlers[payload.name] = function(source, args)
            Aldivine.Events.emit('commands:server:invoked', {
                command = payload.name,
                source = source,
                args = args,
            })
        end
    end
end)
