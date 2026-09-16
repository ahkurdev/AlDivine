-- spawn client: asks for a spawn point on start, applies placement.
-- Runs in the sandboxed Lua runtime.

Aldivine.Events.on('spawn:client:start', function(payload)
    Aldivine.Events.emit('spawn:server:request', {})
end)

Aldivine.Events.on('spawn:client:place', function(point)
    if type(point) == 'table' then
        Aldivine.Game.set_position(point.x, point.y, point.z, point.heading)
    end
end)
