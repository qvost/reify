//! Optional loopback facilities for integration tests in restricted runners.
pub async fn loopback_listener() -> Option<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::Unsupported
            ) =>
        {
            eprintln!("skipping network integration test: loopback sockets unavailable ({error})");
            None
        }
        Err(error) => panic!("cannot create integration-test listener: {error}"),
    }
}
