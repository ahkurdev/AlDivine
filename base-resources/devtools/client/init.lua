-- devtools client: client-side developer tools and inspect overlay.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('devtools:client:start', function(payload)
    Aldivine.Events.emit('devtools:server:request_stats', {})
end)

Aldivine.Events.on('devtools:client:stats', function(stats)
    if type(stats) == 'table' then
        Aldivine.UI.show('devtools')
    end
end)

Aldivine.Events.on('devtools:client:set_coords', function(payload)
    if type(payload) == 'table' and payload.coords then
        local c = payload.coords
        Aldivine.Game.set_position(c.x or 0.0, c.y or 0.0, c.z or 72.0, c.heading or 0.0)
    end
end)
