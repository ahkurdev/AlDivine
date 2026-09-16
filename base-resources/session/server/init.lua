-- session server: counts players across connect/disconnect.
-- Runs in the sandboxed Lua runtime.

local players = {}

Aldivine.Events.on('session:server:connecting', function(payload)
    if type(payload) == 'table' and type(payload.player) == 'string' then
        players[payload.player] = true
        Aldivine.Events.emit('session:server:count', { count = Aldivine.Session.player_count(players) })
    end
end)

Aldivine.Events.on('session:server:disconnecting', function(payload)
    if type(payload) == 'table' and type(payload.player) == 'string' then
        players[payload.player] = nil
        Aldivine.Events.emit('session:server:count', { count = Aldivine.Session.player_count(players) })
    end
end)
