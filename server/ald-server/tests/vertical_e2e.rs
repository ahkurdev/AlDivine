use ald_config::Config;
use ald_server::Server;

fn free_port() -> u16 {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").expect("probe socket");
    s.local_addr().expect("probe addr").port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vertical_join_reaches_running_and_registers_framework_player() {
    let port = free_port();
    let mut cfg = Config::default();
    cfg.network.bind = format!("127.0.0.1:{port}");
    let server = Server::start(cfg).await.expect("server must start");

    // Give the listener + supervisor a moment to come up.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let mut client = astryn_core::AstrynClient::start(astryn_core::ClientConfig {
        server_addr: format!("127.0.0.1:{port}"),
        ..Default::default()
    });

    let mut running = false;
    for _ in 0..150 {
        if client.state() == astryn_core::ClientState::Running {
            running = true;
            break;
        }
        if client.state() == astryn_core::ClientState::Failed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(running, "client must reach Running, got {:?}", client.state());
    assert!(client.has_session().await, "client must retain its session socket");

    let ev = client.last_event().expect("server welcome event must arrive");
    assert_eq!(ev.event, "ald:server:welcome");
    let body = ev.json_data().expect("welcome payload is JSON");
    assert!(body.get("motd").is_some());

    let online = server.state().framework_players().lock().await.online_count();
    assert!(online >= 1, "framework must hold the joined player");

    let rel = "spawn/ald_manifest.toml";
    let disk = std::fs::read(format!("../../base-resources/{rel}")).expect("fixture on disk");
    let expected =
        ald_protocol::ResourceEntry { name: rel.into(), hash: ald_cache::sha256_hex(&disk), size: disk.len() as u64 };
    let cache_dir = std::env::temp_dir().join(format!("ald-e2e-cache-{}", std::process::id()));
    let got = client.fetch_resource_file(rel, &expected, &cache_dir).await.expect("download must succeed");
    assert_eq!(got, disk, "downloaded bytes must match the server file");
    let cache = ald_cache::ContentCache::open(&cache_dir, 64 * 1024 * 1024).expect("cache opens");
    assert!(cache.verify(&expected.hash).expect("verify runs"), "cache must verify the committed object");
    std::fs::remove_dir_all(&cache_dir).ok();

    let sessions = server.state().admission().lock().await.session_count();
    assert!(sessions >= 1);

    #[cfg(feature = "lua")]
    {
        let mut running = false;
        for _ in 0..100 {
            if server.state().running_count().await >= 1 {
                running = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(running, "base resources must reach Running with the lua host");
    }

    client.shutdown().await.unwrap();
    server.shutdown().await.unwrap();
}
