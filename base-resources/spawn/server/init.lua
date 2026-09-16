-- spawn server: tracks spawn points and answers spawn requests.
-- Runs in the sandboxed Lua runtime. Capabilities come from ald_manifest.toml.

local spawn_points = {
    { x = 0.0, y = 0.0, z = 72.0, heading = 0.0 },
}

Aldivine.Events.on('spawn:server:request', function(payload)
    local point = spawn_points[1]
    Aldivine.Events.emit('spawn:client:place', point)
end)

Aldivine.Events.on('spawn:server:add_point', function(payload)
    if type(payload) == 'table' then
        spawn_points[#spawn_points + 1] = payload
    end
end)
