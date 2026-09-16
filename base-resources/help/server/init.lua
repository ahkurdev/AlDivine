-- help server: provides help topics and command descriptions.
-- Runs in sandboxed Lua runtime.

local topics = {
    help = "Display help topics",
    spawn = "Respawn player character",
    chat = "Send chat message"
}

Aldivine.Events.on('help:server:request', function(payload)
    Aldivine.Events.emit('help:client:response', { topics = topics })
end)
