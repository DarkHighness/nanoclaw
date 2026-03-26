#[path = "../src/process/network_proxy.rs"]
mod network_proxy;

use network_proxy::{DomainAllowlist, ProxyBindTarget, ProxyConfig, ProxyManager};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::{os::unix::net::UnixStream, path::PathBuf};
use tempfile::tempdir;

#[test]
fn socks5h_proxy_allows_listed_hostname_and_relays_data() {
    let (target_addr, _target_worker) = spawn_echo_server();
    let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
    assert_eq!(allowlist.domains(), &["localhost".to_string()]);
    let mut proxy = ProxyManager::start(ProxyConfig::localhost(allowlist)).unwrap();
    let proxy_addr = proxy.endpoint().bind_tcp_addr().unwrap();

    let mut stream = connect_over_socks5h(proxy_addr, "localhost", target_addr.port()).unwrap();
    stream.write_all(b"ping").unwrap();
    let mut response = [0u8; 4];
    stream.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"pong");

    proxy.shutdown().unwrap();
}

#[test]
fn socks5h_proxy_rejects_hostname_outside_allowlist() {
    let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
    let mut proxy = ProxyManager::start(ProxyConfig::localhost(allowlist)).unwrap();
    let proxy_addr = proxy.endpoint().bind_tcp_addr().unwrap();

    let reply = socks5_domain_reply_code(proxy_addr, "not-allowed.invalid", 443).unwrap();
    assert_eq!(reply, 0x02);

    proxy.shutdown().unwrap();
}

#[test]
fn socks5h_proxy_rejects_ip_literal_requests() {
    let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
    let mut proxy = ProxyManager::start(ProxyConfig::localhost(allowlist)).unwrap();
    let proxy_addr = proxy.endpoint().bind_tcp_addr().unwrap();

    let reply = socks5_ipv4_reply_code(proxy_addr, [127, 0, 0, 1], 443).unwrap();
    assert_eq!(reply, 0x08);

    proxy.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn unix_socket_proxy_endpoint_enforces_allowlist() {
    let socket_dir = tempdir().unwrap();
    let socket_path = PathBuf::from(socket_dir.path()).join("proxy.sock");
    let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
    let mut proxy = ProxyManager::start(ProxyConfig {
        allowlist,
        bind: ProxyBindTarget::UnixSocket(socket_path.clone()),
    })
    .unwrap();

    let reply =
        socks5_domain_reply_code_over_unix(&socket_path, "not-allowed.invalid", 443).unwrap();
    assert_eq!(reply, 0x02);

    proxy.shutdown().unwrap();
}

#[test]
fn socks5h_proxy_handles_parallel_clients_without_cross_talk() {
    let connection_count = 24;
    let (target_addr, _target_worker) = spawn_parallel_echo_server(connection_count);
    let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
    let mut proxy = ProxyManager::start(ProxyConfig::localhost(allowlist)).unwrap();
    let proxy_addr = proxy.endpoint().bind_tcp_addr().unwrap();
    let start_barrier = Arc::new(Barrier::new(connection_count));
    let mut workers = Vec::new();

    for index in 0..connection_count {
        let start_barrier = Arc::clone(&start_barrier);
        workers.push(thread::spawn(move || {
            start_barrier.wait();
            let mut stream = connect_over_socks5h(proxy_addr, "localhost", target_addr.port())
                .expect("proxy connection should succeed");
            let payload = format!("{index:04}");
            stream
                .write_all(payload.as_bytes())
                .expect("client payload write should succeed");
            let mut response = [0u8; 4];
            stream
                .read_exact(&mut response)
                .expect("client response should succeed");
            assert_eq!(response, *b"pong");
        }));
    }

    for worker in workers {
        worker.join().unwrap();
    }

    proxy.shutdown().unwrap();
}

fn spawn_echo_server() -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    let addr = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().unwrap();
        let mut payload = [0u8; 4];
        stream.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, b"ping");
        stream.write_all(b"pong").unwrap();
    });
    (addr, worker)
}

fn spawn_parallel_echo_server(expected_connections: usize) -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    let addr = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let mut connection_workers = Vec::new();
        for _ in 0..expected_connections {
            let (mut stream, _peer) = listener.accept().unwrap();
            connection_workers.push(thread::spawn(move || {
                let mut payload = [0u8; 4];
                stream.read_exact(&mut payload).unwrap();
                thread::sleep(Duration::from_millis(10));
                stream.write_all(b"pong").unwrap();
            }));
        }
        for worker in connection_workers {
            worker.join().unwrap();
        }
    });
    (addr, worker)
}

fn connect_over_socks5h(
    proxy_addr: SocketAddr,
    host: &str,
    port: u16,
) -> std::io::Result<TcpStream> {
    let mut stream = connect_with_retry(proxy_addr)?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "proxy rejected method negotiation",
        ));
    }

    let host_bytes = host.as_bytes();
    let mut request = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    request.extend(host_bytes);
    request.extend(port.to_be_bytes());
    stream.write_all(&request)?;

    let reply = read_socks_reply_code(&mut stream)?;
    if reply != 0x00 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("proxy returned failure code {reply:#x}"),
        ));
    }
    Ok(stream)
}

fn socks5_domain_reply_code(proxy_addr: SocketAddr, host: &str, port: u16) -> std::io::Result<u8> {
    let mut stream = connect_with_retry(proxy_addr)?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "proxy rejected method negotiation",
        ));
    }
    let host_bytes = host.as_bytes();
    let mut request = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    request.extend(host_bytes);
    request.extend(port.to_be_bytes());
    stream.write_all(&request)?;
    read_socks_reply_code(&mut stream)
}

fn socks5_ipv4_reply_code(
    proxy_addr: SocketAddr,
    address: [u8; 4],
    port: u16,
) -> std::io::Result<u8> {
    let mut stream = connect_with_retry(proxy_addr)?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "proxy rejected method negotiation",
        ));
    }
    let mut request = vec![0x05, 0x01, 0x00, 0x01];
    request.extend(address);
    request.extend(port.to_be_bytes());
    stream.write_all(&request)?;
    read_socks_reply_code(&mut stream)
}

#[cfg(unix)]
fn socks5_domain_reply_code_over_unix(
    socket_path: &std::path::Path,
    host: &str,
    port: u16,
) -> std::io::Result<u8> {
    let mut stream = connect_unix_with_retry(socket_path)?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "proxy rejected method negotiation",
        ));
    }
    let host_bytes = host.as_bytes();
    let mut request = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    request.extend(host_bytes);
    request.extend(port.to_be_bytes());
    stream.write_all(&request)?;
    read_socks_reply_code(&mut stream)
}

fn read_socks_reply_code(stream: &mut impl Read) -> std::io::Result<u8> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    if header[0] != 0x05 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid socks reply version",
        ));
    }
    match header[3] {
        0x01 => {
            let mut skip = [0u8; 6];
            stream.read_exact(&mut skip)?;
        }
        0x04 => {
            let mut skip = [0u8; 18];
            stream.read_exact(&mut skip)?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            let mut skip = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut skip)?;
        }
        _ => {}
    }
    Ok(header[1])
}

fn connect_with_retry(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for _ in 0..20 {
        match TcpStream::connect(addr) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "proxy did not become reachable",
        )
    }))
}

#[cfg(unix)]
fn connect_unix_with_retry(path: &std::path::Path) -> std::io::Result<UnixStream> {
    let mut last_error = None;
    for _ in 0..20 {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "unix proxy did not become reachable",
        )
    }))
}
