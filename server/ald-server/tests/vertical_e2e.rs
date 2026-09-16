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

    let online = server.state().framework_players().lock().await.online_count();
    assert!(online >= 1, "framework must hold the joined player");

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
