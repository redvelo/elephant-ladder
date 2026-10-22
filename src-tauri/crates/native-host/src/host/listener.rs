//! Loopback sockets: one per origin the host serves.
//!
//! Each plane gets its own listener so a publication can never be reached on the
//! trusted reader's port, and the installation's ports are remembered so a
//! reopened publication keeps the origin its stored data belongs to.

use std::net::TcpListener;

use axum::Router;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::oneshot;

use crate::storage;

pub struct RunningServer {
    pub(super) shutdown: Option<oneshot::Sender<()>>,
    _task: tokio::task::JoinHandle<()>,
}

pub fn bind_listener(authority: &str) -> Result<TcpListener, String> {
    let address = authority
        .parse::<std::net::SocketAddr>()
        .map_err(|error| format!("could not parse {authority}: {error}"))?;
    let socket = Socket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(|error| format!("could not create listener for {authority}: {error}"))?;
    #[cfg(unix)]
    socket.set_reuse_address(true).map_err(|error| {
        format!("could not configure reusable listener for {authority}: {error}")
    })?;
    socket
        .bind(&address.into())
        .map_err(|error| format!("could not bind {authority}: {error}"))?;
    socket
        .listen(128)
        .map_err(|error| format!("could not listen on {authority}: {error}"))?;
    let listener = TcpListener::from(socket);
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("could not configure {authority}: {error}"))?;
    Ok(listener)
}

pub(crate) fn bind_installation_origin_listeners(
    storage: &mut storage::Storage,
) -> Result<(TcpListener, TcpListener), String> {
    let configuration = storage.installation_origin_config()?;
    let reader = bind_listener(&format!(
        "127.0.0.1:{}",
        configuration
            .and_then(|value| value.reader_port)
            .unwrap_or(0)
    ))
    .map_err(|error| {
        if let Some(port) = configuration.and_then(|value| value.reader_port) {
            format!("could not bind persisted reader port {port}: {error}")
        } else {
            error
        }
    })?;
    let imported_publication = bind_listener(&format!(
        "127.0.0.1:{}",
        configuration
            .map(|value| value.foreground_port)
            .unwrap_or(0)
    ))
    .map_err(|error| {
        if let Some(configuration) = configuration {
            format!(
                "could not bind persisted imported-publication port {}: {error}",
                configuration.foreground_port
            )
        } else {
            error
        }
    })?;
    let reader_port = reader
        .local_addr()
        .map_err(|error| format!("could not inspect reader listener: {error}"))?
        .port();
    let foreground_port = imported_publication
        .local_addr()
        .map_err(|error| format!("could not inspect imported-publication listener: {error}"))?
        .port();
    storage.initialize_origin_ports(foreground_port, reader_port)?;
    Ok((reader, imported_publication))
}

pub fn spawn_server(listener: TcpListener, router: Router, name: &'static str) -> RunningServer {
    let (shutdown, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .unwrap_or_else(|error| panic!("could not attach {name} listener to Tokio: {error}"));
        if axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .is_err()
        {
            tracing::error!(event = "http_server_failed", server_role = name);
        }
    });
    RunningServer {
        shutdown: Some(shutdown),
        _task: task,
    }
}
