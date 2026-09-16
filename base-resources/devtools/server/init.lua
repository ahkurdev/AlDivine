-- devtools server: admin/developer diagnostics and entity teleport/inspection.
-- Runs in sandboxed Lua runtime.

Aldivine.Events.on('devtools:server:teleport', function(payload)
    if type(payload) == 'table' and payload.target and payload.coords then
        Aldivine.Events.emit('devtools:client:set_coords', payload)
    end
end)

Aldivine.Events.on('devtools:server:request_stats', function(payload)
    Aldivine.Events.emit('devtools:client:stats', {
        server_uptime = 0,
        resources_active = 7,
    })
end)
