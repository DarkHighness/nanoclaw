use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[cfg(unix)]
use std::{
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

const SOCKS_VERSION: u8 = 0x05;
const SOCKS_METHOD_NO_AUTH: u8 = 0x00;
const SOCKS_METHOD_NOT_ACCEPTABLE: u8 = 0xFF;
const SOCKS_CMD_CONNECT: u8 = 0x01;
const SOCKS_ATYP_IPV4: u8 = 0x01;
const SOCKS_ATYP_DOMAIN: u8 = 0x03;
const SOCKS_ATYP_IPV6: u8 = 0x04;

const SOCKS_REPLY_SUCCESS: u8 = 0x00;
const SOCKS_REPLY_GENERAL_FAILURE: u8 = 0x01;
const SOCKS_REPLY_CONNECTION_NOT_ALLOWED: u8 = 0x02;
const SOCKS_REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;
const MAX_CONCURRENT_PROXY_CONNECTIONS: usize = 128;

#[derive(Debug)]
pub enum ProxyError {
    InvalidAllowlistEntry(String),
    EmptyAllowlist,
    Bind(io::Error),
    Join,
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAllowlistEntry(value) => {
                write!(f, "invalid allowlist domain entry: {value}")
            }
            Self::EmptyAllowlist => write!(f, "domain allowlist cannot be empty"),
            Self::Bind(error) => write!(f, "failed to bind proxy listener: {error}"),
            Self::Join => write!(f, "failed to join proxy worker thread"),
        }
    }
}

impl std::error::Error for ProxyError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    pub fn new(domains: impl IntoIterator<Item = String>) -> Result<Self, ProxyError> {
        let mut normalized = Vec::new();
        for raw in domains {
            let domain = normalize_domain(&raw)
                .ok_or_else(|| ProxyError::InvalidAllowlistEntry(raw.clone()))?;
            if !normalized.iter().any(|existing| existing == &domain) {
                normalized.push(domain);
            }
        }
        if normalized.is_empty() {
            return Err(ProxyError::EmptyAllowlist);
        }
        Ok(Self {
            domains: normalized,
        })
    }

    pub fn allows_host(&self, host: &str) -> bool {
        let Some(normalized_host) = normalize_domain(host) else {
            return false;
        };
        if IpAddr::from_str(&normalized_host).is_ok() {
            return false;
        }
        self.domains.iter().any(|allowed| {
            normalized_host == *allowed
                || normalized_host
                    .strip_suffix(allowed)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
    }

    #[allow(dead_code)]
    pub fn domains(&self) -> &[String] {
        &self.domains
    }
}

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub allowlist: DomainAllowlist,
    pub bind: ProxyBindTarget,
}

impl ProxyConfig {
    #[allow(dead_code)]
    pub fn localhost(allowlist: DomainAllowlist) -> Self {
        Self {
            allowlist,
            bind: ProxyBindTarget::LocalhostTcp(SocketAddr::from(([127, 0, 0, 1], 0))),
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum ProxyBindTarget {
    LocalhostTcp(SocketAddr),
    #[cfg(unix)]
    UnixSocket(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyEndpoint {
    LocalhostTcp(SocketAddr),
    #[cfg(unix)]
    UnixSocket(PathBuf),
}

impl ProxyEndpoint {
    pub fn all_proxy_url(&self) -> Option<String> {
        match self {
            Self::LocalhostTcp(addr) => Some(format!("socks5h://{addr}")),
            #[cfg(unix)]
            Self::UnixSocket(_) => None,
        }
    }

    #[allow(dead_code)]
    pub fn bind_tcp_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::LocalhostTcp(addr) => Some(*addr),
            #[cfg(unix)]
            Self::UnixSocket(_) => None,
        }
    }

    pub fn env_vars(&self) -> BTreeMap<String, String> {
        // Standard proxy environment variables only support URL-style
        // endpoints. Unix socket transport still needs host-specific wiring
        // (for example, bind-mounted sockets plus a local connector).
        self.all_proxy_url().map_or_else(BTreeMap::new, |url| {
            BTreeMap::from([
                ("ALL_PROXY".to_string(), url.clone()),
                ("all_proxy".to_string(), url),
            ])
        })
    }
}

pub struct ProxyHandle {
    endpoint: ProxyEndpoint,
    #[cfg(unix)]
    socket_path: Option<PathBuf>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ProxyHandle {
    pub fn endpoint(&self) -> &ProxyEndpoint {
        &self.endpoint
    }

    #[allow(dead_code)]
    pub fn env_vars(&self) -> BTreeMap<String, String> {
        self.endpoint.env_vars()
    }

    pub fn shutdown(&mut self) -> Result<(), ProxyError> {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| ProxyError::Join)?;
        }
        #[cfg(unix)]
        if let Some(path) = self.socket_path.take() {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub struct ProxyManager;

impl ProxyManager {
    pub fn start(config: ProxyConfig) -> Result<ProxyHandle, ProxyError> {
        match config.bind {
            ProxyBindTarget::LocalhostTcp(bind_addr) => {
                let listener = TcpListener::bind(bind_addr).map_err(ProxyError::Bind)?;
                listener.set_nonblocking(true).map_err(ProxyError::Bind)?;
                let endpoint =
                    ProxyEndpoint::LocalhostTcp(listener.local_addr().map_err(ProxyError::Bind)?);
                let shutdown = Arc::new(AtomicBool::new(false));
                let shutdown_signal = Arc::clone(&shutdown);
                let allowlist = config.allowlist;
                let worker = thread::Builder::new()
                    .name("nanoclaw-network-proxy".to_string())
                    .spawn(move || run_tcp_accept_loop(listener, shutdown_signal, allowlist))
                    .map_err(ProxyError::Bind)?;
                Ok(ProxyHandle {
                    endpoint,
                    #[cfg(unix)]
                    socket_path: None,
                    shutdown,
                    worker: Some(worker),
                })
            }
            #[cfg(unix)]
            ProxyBindTarget::UnixSocket(path) => {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                let listener = UnixListener::bind(&path).map_err(ProxyError::Bind)?;
                listener.set_nonblocking(true).map_err(ProxyError::Bind)?;
                let endpoint = ProxyEndpoint::UnixSocket(path.clone());
                let shutdown = Arc::new(AtomicBool::new(false));
                let shutdown_signal = Arc::clone(&shutdown);
                let allowlist = config.allowlist;
                let worker = thread::Builder::new()
                    .name("nanoclaw-network-proxy".to_string())
                    .spawn(move || run_unix_accept_loop(listener, shutdown_signal, allowlist))
                    .map_err(ProxyError::Bind)?;
                Ok(ProxyHandle {
                    endpoint,
                    socket_path: Some(path),
                    shutdown,
                    worker: Some(worker),
                })
            }
        }
    }
}

struct RetainedProxy {
    endpoint: ProxyEndpoint,
    _handle: ProxyHandle,
}

fn retained_proxies() -> &'static Mutex<BTreeMap<String, RetainedProxy>> {
    static RETAINED: OnceLock<Mutex<BTreeMap<String, RetainedProxy>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[allow(dead_code)]
pub fn start_retained_proxy(config: ProxyConfig) -> Result<ProxyEndpoint, ProxyError> {
    let key = retained_proxy_key(&config);
    let mut retained = retained_proxies()
        .lock()
        .expect("retained proxy registry poisoned");
    if let Some(proxy) = retained.get(&key) {
        return Ok(proxy.endpoint.clone());
    }

    // Retained proxies are process-scoped infrastructure, not per-command
    // children. Starting them under the registry lock avoids a race where two
    // concurrent callers both bind the same Unix-socket path and the loser
    // tears down the winner's public socket file on drop.
    let handle = ProxyManager::start(config)?;
    let endpoint = handle.endpoint().clone();
    retained.insert(
        key,
        RetainedProxy {
            endpoint: endpoint.clone(),
            _handle: handle,
        },
    );
    Ok(endpoint)
}

fn retained_proxy_key(config: &ProxyConfig) -> String {
    let domains = config.allowlist.domains().join(",");
    match &config.bind {
        ProxyBindTarget::LocalhostTcp(_) => format!("tcp:{domains}"),
        #[cfg(unix)]
        ProxyBindTarget::UnixSocket(path) => format!("unix:{}:{domains}", path.display()),
    }
}

fn run_tcp_accept_loop(
    listener: TcpListener,
    shutdown: Arc<AtomicBool>,
    allowlist: DomainAllowlist,
) {
    let active_connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _peer)) => {
                let Some(connection_permit) = try_acquire_connection_slot(&active_connections)
                else {
                    // Saturation must fail closed. Dropping the connection
                    // immediately keeps the proxy from amplifying a connection
                    // flood into unbounded relay threads.
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                };
                let allowlist = allowlist.clone();
                let _ = thread::Builder::new()
                    .name("nanoclaw-network-proxy-client".to_string())
                    .spawn(move || {
                        let _permit = connection_permit;
                        let _ = handle_tcp_client(client, &allowlist);
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(unix)]
fn run_unix_accept_loop(
    listener: UnixListener,
    shutdown: Arc<AtomicBool>,
    allowlist: DomainAllowlist,
) {
    let active_connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _peer)) => {
                let Some(connection_permit) = try_acquire_connection_slot(&active_connections)
                else {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                };
                let allowlist = allowlist.clone();
                let _ = thread::Builder::new()
                    .name("nanoclaw-network-proxy-client".to_string())
                    .spawn(move || {
                        let _permit = connection_permit;
                        let _ = handle_unix_client(client, &allowlist);
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

struct ConnectionPermit {
    active_connections: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

fn try_acquire_connection_slot(
    active_connections: &Arc<std::sync::atomic::AtomicUsize>,
) -> Option<ConnectionPermit> {
    let mut current = active_connections.load(Ordering::Relaxed);
    loop {
        if current >= MAX_CONCURRENT_PROXY_CONNECTIONS {
            return None;
        }
        match active_connections.compare_exchange_weak(
            current,
            current + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                return Some(ConnectionPermit {
                    active_connections: Arc::clone(active_connections),
                });
            }
            Err(updated) => current = updated,
        }
    }
}

fn handle_tcp_client(mut client: TcpStream, allowlist: &DomainAllowlist) -> io::Result<()> {
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(Duration::from_secs(10)))?;
    client.set_write_timeout(Some(Duration::from_secs(10)))?;
    negotiate_method(&mut client)?;
    let request = read_connect_request(&mut client)?;

    if !allowlist.allows_host(&request.host) {
        write_reply(
            &mut client,
            SOCKS_REPLY_CONNECTION_NOT_ALLOWED,
            SocketAddr::from(([0, 0, 0, 0], 0)),
        )?;
        return Ok(());
    }

    let upstream = match TcpStream::connect((request.host.as_str(), request.port)) {
        Ok(stream) => stream,
        Err(_) => {
            write_reply(
                &mut client,
                SOCKS_REPLY_GENERAL_FAILURE,
                SocketAddr::from(([0, 0, 0, 0], 0)),
            )?;
            return Ok(());
        }
    };

    let bound = upstream
        .local_addr()
        .unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
    write_reply(&mut client, SOCKS_REPLY_SUCCESS, bound)?;
    client.set_read_timeout(None)?;
    client.set_write_timeout(None)?;
    relay_tcp_bidirectional(client, upstream)
}

#[cfg(unix)]
fn handle_unix_client(mut client: UnixStream, allowlist: &DomainAllowlist) -> io::Result<()> {
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(Duration::from_secs(10)))?;
    client.set_write_timeout(Some(Duration::from_secs(10)))?;
    negotiate_method(&mut client)?;
    let request = read_connect_request(&mut client)?;

    if !allowlist.allows_host(&request.host) {
        write_reply(
            &mut client,
            SOCKS_REPLY_CONNECTION_NOT_ALLOWED,
            SocketAddr::from(([0, 0, 0, 0], 0)),
        )?;
        return Ok(());
    }

    let upstream = match TcpStream::connect((request.host.as_str(), request.port)) {
        Ok(stream) => stream,
        Err(_) => {
            write_reply(
                &mut client,
                SOCKS_REPLY_GENERAL_FAILURE,
                SocketAddr::from(([0, 0, 0, 0], 0)),
            )?;
            return Ok(());
        }
    };

    let bound = upstream
        .local_addr()
        .unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
    write_reply(&mut client, SOCKS_REPLY_SUCCESS, bound)?;
    client.set_read_timeout(None)?;
    client.set_write_timeout(None)?;
    relay_unix_bidirectional(client, upstream)
}

fn negotiate_method(stream: &mut impl ReadWriteStream) -> io::Result<()> {
    let mut greeting = [0u8; 2];
    stream.read_exact(&mut greeting)?;
    if greeting[0] != SOCKS_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected socks version",
        ));
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    stream.read_exact(&mut methods)?;
    let selected = if methods.iter().any(|method| *method == SOCKS_METHOD_NO_AUTH) {
        SOCKS_METHOD_NO_AUTH
    } else {
        SOCKS_METHOD_NOT_ACCEPTABLE
    };
    stream.write_all(&[SOCKS_VERSION, selected])?;
    if selected == SOCKS_METHOD_NOT_ACCEPTABLE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "no supported authentication method",
        ));
    }
    Ok(())
}

struct ConnectRequest {
    host: String,
    port: u16,
}

fn read_connect_request(stream: &mut impl ReadWriteStream) -> io::Result<ConnectRequest> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    if header[0] != SOCKS_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected socks version in request",
        ));
    }
    if header[1] != SOCKS_CMD_CONNECT {
        write_reply(
            stream,
            SOCKS_REPLY_COMMAND_NOT_SUPPORTED,
            SocketAddr::from(([0, 0, 0, 0], 0)),
        )?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only CONNECT is supported",
        ));
    }

    match header[3] {
        SOCKS_ATYP_DOMAIN => {
            let mut len_buf = [0u8; 1];
            stream.read_exact(&mut len_buf)?;
            let mut host_buf = vec![0u8; len_buf[0] as usize];
            stream.read_exact(&mut host_buf)?;
            let host = String::from_utf8_lossy(&host_buf).to_string();
            let port = read_port(stream)?;
            Ok(ConnectRequest { host, port })
        }
        SOCKS_ATYP_IPV4 | SOCKS_ATYP_IPV6 => {
            let addr_len = if header[3] == SOCKS_ATYP_IPV4 { 4 } else { 16 };
            let mut skip = vec![0u8; addr_len];
            stream.read_exact(&mut skip)?;
            let _ = read_port(stream)?;
            write_reply(
                stream,
                SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
                SocketAddr::from(([0, 0, 0, 0], 0)),
            )?;
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "ip-literal target requests are not supported",
            ))
        }
        _ => {
            write_reply(
                stream,
                SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
                SocketAddr::from(([0, 0, 0, 0], 0)),
            )?;
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported target address type",
            ))
        }
    }
}

fn read_port(stream: &mut impl ReadWriteStream) -> io::Result<u16> {
    let mut port_buf = [0u8; 2];
    stream.read_exact(&mut port_buf)?;
    Ok(u16::from_be_bytes(port_buf))
}

fn write_reply(stream: &mut impl ReadWriteStream, code: u8, bound: SocketAddr) -> io::Result<()> {
    match bound.ip() {
        IpAddr::V4(ip) => {
            let mut response = vec![SOCKS_VERSION, code, 0x00, SOCKS_ATYP_IPV4];
            response.extend(ip.octets());
            response.extend(bound.port().to_be_bytes());
            stream.write_all(&response)
        }
        IpAddr::V6(ip) => {
            let mut response = vec![SOCKS_VERSION, code, 0x00, SOCKS_ATYP_IPV6];
            response.extend(ip.octets());
            response.extend(bound.port().to_be_bytes());
            stream.write_all(&response)
        }
    }
}

fn relay_tcp_bidirectional(mut client: TcpStream, mut upstream: TcpStream) -> io::Result<()> {
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let uplink = thread::spawn(move || {
        let _ = io::copy(&mut client_reader, &mut upstream_writer);
        let _ = upstream_writer.shutdown(Shutdown::Write);
    });

    let _ = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = uplink.join();
    Ok(())
}

#[cfg(unix)]
fn relay_unix_bidirectional(mut client: UnixStream, mut upstream: TcpStream) -> io::Result<()> {
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let uplink = thread::spawn(move || {
        let _ = io::copy(&mut client_reader, &mut upstream_writer);
        let _ = upstream_writer.shutdown(Shutdown::Write);
    });

    let _ = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = uplink.join();
    Ok(())
}

trait ReadWriteStream: Read + Write {}
impl<T: Read + Write> ReadWriteStream for T {}

fn normalize_domain(value: &str) -> Option<String> {
    let normalized = value.trim().trim_matches('.').to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 253 {
        return None;
    }
    if normalized.bytes().any(|byte| {
        !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.')
    }) {
        return None;
    }
    if normalized.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .any(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }) {
        return None;
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::{
        DomainAllowlist, ProxyBindTarget, ProxyConfig, ProxyEndpoint, ProxyManager,
        start_retained_proxy,
    };
    use std::io::{Read, Write};
    use std::net::SocketAddr;
    #[cfg(unix)]
    use std::{os::unix::net::UnixStream, path::PathBuf};
    use std::{
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tempfile::tempdir;

    #[test]
    fn allowlist_matches_exact_and_subdomains() {
        let allowlist = DomainAllowlist::new(vec!["example.com".to_string()]).unwrap();
        assert!(allowlist.allows_host("example.com"));
        assert!(allowlist.allows_host("api.example.com"));
        assert!(!allowlist.allows_host("badexample.com"));
    }

    #[test]
    fn allowlist_rejects_ip_literals() {
        let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
        assert!(!allowlist.allows_host("127.0.0.1"));
        assert!(!allowlist.allows_host("::1"));
    }

    #[test]
    fn tcp_endpoint_exports_proxy_env_vars() {
        let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
        let mut handle = ProxyManager::start(ProxyConfig {
            allowlist,
            bind: ProxyBindTarget::LocalhostTcp(SocketAddr::from(([127, 0, 0, 1], 0))),
        })
        .unwrap();
        let env_vars = handle.env_vars();
        assert!(env_vars.contains_key("ALL_PROXY"));
        assert!(env_vars.contains_key("all_proxy"));
        handle.shutdown().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_endpoint_does_not_export_url_proxy_env_vars() {
        let allowlist = DomainAllowlist::new(vec!["localhost".to_string()]).unwrap();
        let dir = tempdir().unwrap();
        let socket_path = PathBuf::from(dir.path()).join("proxy.sock");
        let mut handle = ProxyManager::start(ProxyConfig {
            allowlist,
            bind: ProxyBindTarget::UnixSocket(socket_path),
        })
        .unwrap();
        assert!(handle.env_vars().is_empty());
        handle.shutdown().unwrap();
    }

    #[test]
    fn retained_proxy_parallel_tcp_starts_reuse_one_endpoint() {
        let domain = unique_test_domain("parallel-retained");
        let config = ProxyConfig::localhost(DomainAllowlist::new(vec![domain]).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let barrier = std::sync::Arc::clone(&barrier);
            let config = config.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                start_retained_proxy(config).unwrap()
            }));
        }

        let endpoints = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let first = endpoints.first().cloned().unwrap();
        assert!(endpoints.iter().all(|endpoint| endpoint == &first));
    }

    #[cfg(unix)]
    #[test]
    fn retained_proxy_parallel_unix_starts_keep_socket_reachable() {
        let socket_dir = tempdir().unwrap();
        let socket_path = PathBuf::from(socket_dir.path()).join("parallel.sock");
        let domain = unique_test_domain("parallel-unix");
        let config = ProxyConfig {
            allowlist: DomainAllowlist::new(vec![domain]).unwrap(),
            bind: ProxyBindTarget::UnixSocket(socket_path.clone()),
        };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let barrier = std::sync::Arc::clone(&barrier);
            let config = config.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                start_retained_proxy(config).unwrap()
            }));
        }

        let endpoints = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let first = endpoints.first().cloned().unwrap();
        assert!(endpoints.iter().all(|endpoint| endpoint == &first));
        assert_eq!(first, ProxyEndpoint::UnixSocket(socket_path.clone()));

        let mut last_error = None;
        for _ in 0..20 {
            match UnixStream::connect(&socket_path) {
                Ok(mut stream) => {
                    stream.write_all(&[0x05, 0x01, 0x00]).unwrap();
                    let mut method = [0u8; 2];
                    stream.read_exact(&mut method).unwrap();
                    assert_eq!(method, [0x05, 0x00]);
                    return;
                }
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }

        panic!("retained unix proxy socket was not reachable: {last_error:?}");
    }

    fn unique_test_domain(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{prefix}-{nanos}.example.test")
    }
}
