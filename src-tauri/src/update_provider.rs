use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::ready;
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::Url;
#[cfg(not(target_os = "macos"))]
use tokio::net::lookup_host;
#[cfg(not(target_os = "macos"))]
use tokio::time::timeout;

use crate::update_manifest::{parse_manifest, UpdateManifest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicAddressError {
    Resolve,
    Unsafe,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicAddressResolveError {
    Resolve,
    Timeout,
}

pub(crate) trait PublicAddressResolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, ()>;

    fn resolve_with_deadline(
        &self,
        _host: &str,
        _port: u16,
        _deadline: Instant,
    ) -> Result<Vec<SocketAddr>, PublicAddressResolveError> {
        // A synchronous resolver must not be called from the manifest
        // deadline path. Implementations that can resolve without blocking
        // indefinitely must override this method.
        Err(PublicAddressResolveError::Resolve)
    }
}

#[cfg(target_os = "macos")]
mod macos_system_dns {
    use super::{Instant, PublicAddressResolveError, SocketAddr};
    use std::ffi::CString;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::os::raw::{c_char, c_void};
    use std::ptr;

    const DNS_SERVICE_FLAGS_MORE_COMING: u32 = 0x1;
    const DNS_SERVICE_PROTOCOL_IPV4: u32 = 0x1;
    const DNS_SERVICE_PROTOCOL_IPV6: u32 = 0x2;
    const DNS_SERVICE_NO_ERROR: i32 = 0;

    #[repr(C)]
    struct DnsServiceRefOpaque {
        _private: [u8; 0],
    }

    type DnsServiceRef = *mut DnsServiceRefOpaque;

    #[derive(Default)]
    struct LookupState {
        addresses: Vec<SocketAddr>,
        failed: bool,
        complete: bool,
        port: u16,
    }

    struct DnsServiceRefGuard(DnsServiceRef);

    impl Drop for DnsServiceRefGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // The query is explicitly terminated on every return path.
                unsafe { DNSServiceRefDeallocate(self.0) };
            }
        }
    }

    #[link(name = "System")]
    extern "C" {
        fn DNSServiceGetAddrInfo(
            sd_ref: *mut DnsServiceRef,
            flags: u32,
            interface_index: u32,
            protocol: u32,
            hostname: *const c_char,
            callback: unsafe extern "C" fn(
                DnsServiceRef,
                u32,
                u32,
                i32,
                *const c_char,
                *const libc::sockaddr,
                u32,
                *mut c_void,
            ),
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd_ref: DnsServiceRef) -> libc::c_int;
        fn DNSServiceProcessResult(sd_ref: DnsServiceRef) -> i32;
        fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
    }

    unsafe extern "C" fn address_callback(
        _sd_ref: DnsServiceRef,
        flags: u32,
        _interface_index: u32,
        error_code: i32,
        _hostname: *const c_char,
        address: *const libc::sockaddr,
        _ttl: u32,
        context: *mut c_void,
    ) {
        let state = &mut *(context.cast::<LookupState>());
        if error_code != DNS_SERVICE_NO_ERROR || address.is_null() {
            state.failed = true;
        } else {
            let family = (*address).sa_family as libc::c_int;
            let socket_address = if family == libc::AF_INET {
                let address = ptr::read_unaligned(address.cast::<libc::sockaddr_in>());
                let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr).to_be_bytes());
                Some(SocketAddr::new(ip.into(), state.port))
            } else if family == libc::AF_INET6 {
                let address = ptr::read_unaligned(address.cast::<libc::sockaddr_in6>());
                let ip = Ipv6Addr::from(address.sin6_addr.s6_addr);
                Some(SocketAddr::new(ip.into(), state.port))
            } else {
                None
            };

            if let Some(socket_address) = socket_address {
                if !state.addresses.contains(&socket_address) {
                    state.addresses.push(socket_address);
                }
            } else {
                state.failed = true;
            }
        }

        if flags & DNS_SERVICE_FLAGS_MORE_COMING == 0 {
            state.complete = true;
        }
    }

    fn poll_timeout(deadline: Instant) -> libc::c_int {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let milliseconds = remaining.as_millis().saturating_add(1);
        milliseconds.min(libc::c_int::MAX as u128) as libc::c_int
    }

    pub(super) fn resolve(
        host: &str,
        port: u16,
        deadline: Instant,
    ) -> Result<Vec<SocketAddr>, PublicAddressResolveError> {
        if Instant::now() >= deadline {
            return Err(PublicAddressResolveError::Timeout);
        }
        let hostname = CString::new(host).map_err(|_| PublicAddressResolveError::Resolve)?;
        let mut state = LookupState {
            port,
            ..LookupState::default()
        };
        let mut service_ref = ptr::null_mut();
        let error = unsafe {
            DNSServiceGetAddrInfo(
                &mut service_ref,
                0,
                0,
                DNS_SERVICE_PROTOCOL_IPV4 | DNS_SERVICE_PROTOCOL_IPV6,
                hostname.as_ptr(),
                address_callback,
                (&mut state as *mut LookupState).cast::<c_void>(),
            )
        };
        if error != DNS_SERVICE_NO_ERROR || service_ref.is_null() {
            return Err(PublicAddressResolveError::Resolve);
        }
        let service_ref = DnsServiceRefGuard(service_ref);
        if Instant::now() >= deadline {
            return Err(PublicAddressResolveError::Timeout);
        }
        let fd = unsafe { DNSServiceRefSockFD(service_ref.0) };
        if fd < 0 {
            return Err(PublicAddressResolveError::Resolve);
        }

        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(PublicAddressResolveError::Resolve);
        }

        while !state.complete {
            if Instant::now() >= deadline {
                return Err(PublicAddressResolveError::Timeout);
            }
            let mut poll_fd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let poll_result = unsafe { libc::poll(&mut poll_fd, 1, poll_timeout(deadline)) };
            if poll_result < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PublicAddressResolveError::Resolve);
            }
            if poll_result == 0 {
                return Err(PublicAddressResolveError::Timeout);
            }
            if Instant::now() >= deadline {
                return Err(PublicAddressResolveError::Timeout);
            }

            let process_result = unsafe { DNSServiceProcessResult(service_ref.0) };
            if process_result != DNS_SERVICE_NO_ERROR {
                return Err(PublicAddressResolveError::Resolve);
            }
        }

        if state.failed || state.addresses.is_empty() {
            Err(PublicAddressResolveError::Resolve)
        } else {
            Ok(state.addresses)
        }
    }
}

pub(crate) struct SystemPublicAddressResolver;

impl PublicAddressResolver for SystemPublicAddressResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, ()> {
        let addresses = resolve_system_addresses(host, port)?;
        if addresses.is_empty() {
            Err(())
        } else {
            Ok(addresses)
        }
    }

    fn resolve_with_deadline(
        &self,
        host: &str,
        port: u16,
        deadline: Instant,
    ) -> Result<Vec<SocketAddr>, PublicAddressResolveError> {
        #[cfg(target_os = "macos")]
        {
            return macos_system_dns::resolve(host, port, deadline);
        }

        #[cfg(not(target_os = "macos"))]
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PublicAddressResolveError::Timeout);
            }

            let result = tauri::async_runtime::block_on(timeout(
                remaining,
                lookup_host((host.to_owned(), port)),
            ));
            match result {
                Ok(Ok(addresses)) => {
                    let addresses = addresses.collect::<Vec<_>>();
                    if addresses.is_empty() {
                        Err(PublicAddressResolveError::Resolve)
                    } else {
                        Ok(addresses)
                    }
                }
                Ok(Err(_)) => Err(PublicAddressResolveError::Resolve),
                Err(_) => Err(PublicAddressResolveError::Timeout),
            }
        }
    }
}

fn resolve_system_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>, ()> {
    (host, port)
        .to_socket_addrs()
        .map_err(|_| ())
        .map(|addresses| addresses.collect::<Vec<_>>())
}

#[derive(Clone)]
pub(crate) struct PinnedDnsResolver {
    addresses: Arc<RwLock<HashMap<String, Vec<SocketAddr>>>>,
}

impl PinnedDnsResolver {
    pub(crate) fn new() -> Self {
        Self {
            addresses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn pin(&self, host: &str, addresses: &[SocketAddr]) -> Result<(), ()> {
        if addresses.is_empty() {
            return Err(());
        }
        let mut pinned = self.addresses.write().map_err(|_| ())?;
        pinned.insert(host.to_ascii_lowercase(), addresses.to_vec());
        Ok(())
    }

    fn addresses_for(&self, host: &str) -> Option<Vec<SocketAddr>> {
        self.addresses
            .read()
            .ok()
            .and_then(|addresses| addresses.get(&host.to_ascii_lowercase()).cloned())
    }
}

impl Resolve for PinnedDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let result: Result<Addrs, Box<dyn std::error::Error + Send + Sync>> =
            match self.addresses.read() {
                Err(_) => Err(Box::new(io::Error::new(
                    io::ErrorKind::Other,
                    "pinned address state unavailable",
                ))),
                Ok(addresses) => addresses
                    .get(&name.as_str().to_ascii_lowercase())
                    .cloned()
                    .map(|addresses| Box::new(addresses.into_iter()) as Addrs)
                    .ok_or_else(|| {
                        Box::new(io::Error::new(
                            io::ErrorKind::NotFound,
                            "host address was not pinned",
                        )) as Box<dyn std::error::Error + Send + Sync>
                    }),
            };
        Box::pin(ready(result))
    }
}

pub(crate) fn validate_public_address(
    url: &Url,
    resolver: &dyn PublicAddressResolver,
) -> Result<Vec<SocketAddr>, PublicAddressError> {
    let host = canonical_host(url).ok_or(PublicAddressError::Resolve)?;
    let port = url
        .port_or_known_default()
        .ok_or(PublicAddressError::Resolve)?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        resolver
            .resolve(&host, port)
            .map_err(|_| PublicAddressError::Resolve)?
    };
    classify_public_addresses(&addresses)?;
    Ok(addresses)
}

pub(crate) fn validate_public_ip_literal(url: &Url) -> Result<(), PublicAddressError> {
    let Some(host) = canonical_host(url) else {
        return Err(PublicAddressError::Resolve);
    };
    if let Ok(ip) = host.parse::<IpAddr>() {
        classify_public_addresses(&[SocketAddr::new(
            ip,
            url.port_or_known_default()
                .ok_or(PublicAddressError::Resolve)?,
        )])?;
    }
    Ok(())
}

pub(crate) fn canonical_host(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(
        host.strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .to_string(),
    )
}

fn classify_public_addresses(addresses: &[SocketAddr]) -> Result<(), PublicAddressError> {
    if addresses.is_empty() {
        return Err(PublicAddressError::Resolve);
    }

    let has_safe = addresses.iter().any(|address| is_public_ip(address.ip()));
    let has_unsafe = addresses.iter().any(|address| !is_public_ip(address.ip()));
    match (has_safe, has_unsafe) {
        (true, false) => Ok(()),
        (true, true) => Err(PublicAddressError::Mixed),
        (false, true) => Err(PublicAddressError::Unsafe),
        (false, false) => Err(PublicAddressError::Resolve),
    }
}

fn validate_public_address_with_deadline(
    url: &Url,
    resolver: &dyn PublicAddressResolver,
    deadline: Instant,
) -> Result<Vec<SocketAddr>, ManifestHttpError> {
    let host = canonical_host(url).ok_or(ManifestHttpError::Network)?;
    let port = url
        .port_or_known_default()
        .ok_or(ManifestHttpError::Network)?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        resolver
            .resolve_with_deadline(&host, port, deadline)
            .map_err(|error| match error {
                PublicAddressResolveError::Resolve => ManifestHttpError::Network,
                PublicAddressResolveError::Timeout => ManifestHttpError::Timeout,
            })?
    };
    classify_public_addresses(&addresses).map_err(|_| ManifestHttpError::Network)?;
    Ok(addresses)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let value = u32::from_be_bytes(ip.octets());
    let special = ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ipv4_prefix(value, 0, 8)
        || ipv4_prefix(value, 0x0a00_0000, 8)
        || ipv4_prefix(value, 0x6440_0000, 10)
        || ipv4_prefix(value, 0x7f00_0000, 8)
        || ipv4_prefix(value, 0xa9fe_0000, 16)
        || ipv4_prefix(value, 0xac10_0000, 12)
        || ipv4_prefix(value, 0xc000_0000, 24)
        || ipv4_prefix(value, 0xc000_0200, 24)
        || ipv4_prefix(value, 0xc058_6300, 24)
        || ipv4_prefix(value, 0xc0a8_0000, 16)
        || ipv4_prefix(value, 0xc612_0000, 15)
        || ipv4_prefix(value, 0xc633_6400, 24)
        || ipv4_prefix(value, 0xcb00_7100, 24)
        || ipv4_prefix(value, 0xe000_0000, 4)
        || ipv4_prefix(value, 0xf000_0000, 4);
    !special
}

fn ipv4_prefix(value: u32, network: u32, prefix_length: u32) -> bool {
    let mask = u32::MAX << (32 - prefix_length);
    value & mask == network & mask
}

fn is_public_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let value = u128::from_be_bytes(ip.octets());
    let special = ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ipv6_prefix(value, 0, 96)
        || ipv6_prefix(value, 0x0000_0000_0000_0000_0000_ffff_0000_0000, 96)
        || ipv6_prefix(value, 0x0100_0000_0000_0000_0000_0000_0000_0000, 64)
        || ipv6_prefix(value, 0x2001_0000_0000_0000_0000_0000_0000_0000, 32)
        || ipv6_prefix(value, 0x2001_0002_0000_0000_0000_0000_0000_0000, 48)
        || ipv6_prefix(value, 0x2001_0003_0000_0000_0000_0000_0000_0000, 32)
        || ipv6_prefix(value, 0x2001_0010_0000_0000_0000_0000_0000_0000, 28)
        || ipv6_prefix(value, 0x2001_0020_0000_0000_0000_0000_0000_0000, 28)
        || ipv6_prefix(value, 0x2001_0db8_0000_0000_0000_0000_0000_0000, 32)
        || ipv6_prefix(value, 0x2002_0000_0000_0000_0000_0000_0000_0000, 16)
        || ipv6_prefix(value, 0x3fff_0000_0000_0000_0000_0000_0000_0000, 20)
        || ipv6_prefix(value, 0xfc00_0000_0000_0000_0000_0000_0000_0000, 7)
        || ipv6_prefix(value, 0xfec0_0000_0000_0000_0000_0000_0000_0000, 10)
        || ipv6_prefix(value, 0xfe80_0000_0000_0000_0000_0000_0000_0000, 10);
    !special
}

fn ipv6_prefix(value: u128, network: u128, prefix_length: u32) -> bool {
    value >> (128 - prefix_length) == network >> (128 - prefix_length)
}

pub(crate) const GITHUB_RELEASES_MANIFEST_URL: &str =
    "https://github.com/Cuetrio/macos/releases/latest/download/cuetrio-update-manifest.json";
pub(crate) const MANIFEST_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const MAX_MANIFEST_REDIRECT_HOPS: usize = 5;
pub(crate) const MAX_MANIFEST_BODY_BYTES: usize = 1_048_576;
pub(crate) const ALLOWED_MANIFEST_CONTENT_TYPES: &[&str] =
    &["application/json", "application/octet-stream"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManifestProviderError {
    Network,
    Timeout,
    Redirect,
    HttpStatus,
    ContentType,
    EmptyResponse,
    ResponseTooLarge,
    InvalidEncoding,
    InvalidJson,
    InvalidManifest,
    Internal,
}

impl ManifestProviderError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Network => "provider-network",
            Self::Timeout => "provider-timeout",
            Self::Redirect => "provider-redirect",
            Self::HttpStatus => "provider-http-status",
            Self::ContentType => "provider-content-type",
            Self::EmptyResponse => "provider-empty-response",
            Self::ResponseTooLarge => "provider-response-too-large",
            Self::InvalidEncoding => "provider-invalid-encoding",
            Self::InvalidJson => "provider-invalid-json",
            Self::InvalidManifest => "provider-invalid-manifest",
            Self::Internal => "provider-internal",
        }
    }
}

impl fmt::Display for ManifestProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ManifestProviderError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManifestHttpError {
    Network,
    Timeout,
    Tls,
    Redirect,
    ContentType,
    ResponseTooLarge,
    Internal,
}

impl From<ManifestHttpError> for ManifestProviderError {
    fn from(error: ManifestHttpError) -> Self {
        match error {
            ManifestHttpError::Network | ManifestHttpError::Tls => Self::Network,
            ManifestHttpError::Timeout => Self::Timeout,
            ManifestHttpError::Redirect => Self::Redirect,
            ManifestHttpError::ContentType => Self::ContentType,
            ManifestHttpError::ResponseTooLarge => Self::ResponseTooLarge,
            ManifestHttpError::Internal => Self::Internal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestHttpRequest {
    pub(crate) url: &'static str,
    pub(crate) timeout: Duration,
    pub(crate) max_redirects: usize,
    pub(crate) max_body_bytes: usize,
    pub(crate) accepted_content_types: &'static [&'static str],
}

impl ManifestHttpRequest {
    fn fixed() -> Self {
        Self {
            url: GITHUB_RELEASES_MANIFEST_URL,
            timeout: MANIFEST_FETCH_TIMEOUT,
            max_redirects: MAX_MANIFEST_REDIRECT_HOPS,
            max_body_bytes: MAX_MANIFEST_BODY_BYTES,
            accepted_content_types: ALLOWED_MANIFEST_CONTENT_TYPES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestRedirect {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestHttpResponse {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) content_length: Option<u64>,
    pub(crate) body: Vec<u8>,
    pub(crate) redirects: Vec<ManifestRedirect>,
    pub(crate) final_url: String,
}

pub(crate) trait ManifestHttpTransport {
    fn get(&self, request: ManifestHttpRequest) -> Result<ManifestHttpResponse, ManifestHttpError>;

    fn validate_url(&self, url: &Url) -> Result<(), ManifestHttpError> {
        validate_public_ip_literal(url).map_err(|_| ManifestHttpError::Network)
    }
}

pub(crate) struct ReqwestManifestHttpTransport {
    client: Client,
    resolver: Arc<dyn PublicAddressResolver>,
    pinned_resolver: PinnedDnsResolver,
}

impl ReqwestManifestHttpTransport {
    pub(crate) fn new() -> Result<Self, ManifestProviderError> {
        Self::with_resolver(Arc::new(SystemPublicAddressResolver))
    }

    fn with_resolver(
        resolver: Arc<dyn PublicAddressResolver>,
    ) -> Result<Self, ManifestProviderError> {
        let pinned_resolver = PinnedDnsResolver::new();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(MANIFEST_FETCH_TIMEOUT)
            .dns_resolver(Arc::new(pinned_resolver.clone()))
            .build()
            .map_err(|_| ManifestProviderError::Internal)?;
        Ok(Self {
            client,
            resolver,
            pinned_resolver,
        })
    }
}

impl ManifestHttpTransport for ReqwestManifestHttpTransport {
    fn validate_url(&self, url: &Url) -> Result<(), ManifestHttpError> {
        let host = canonical_host(url).ok_or(ManifestHttpError::Network)?;
        if host.parse::<IpAddr>().is_ok() {
            return validate_public_ip_literal(url).map_err(|_| ManifestHttpError::Network);
        }

        let addresses = self
            .pinned_resolver
            .addresses_for(&host)
            .ok_or(ManifestHttpError::Network)?;
        classify_public_addresses(&addresses).map_err(|_| ManifestHttpError::Network)
    }

    fn get(&self, request: ManifestHttpRequest) -> Result<ManifestHttpResponse, ManifestHttpError> {
        if !is_fixed_request(&request) {
            return Err(ManifestHttpError::Internal);
        }

        let deadline = Instant::now()
            .checked_add(request.timeout)
            .ok_or(ManifestHttpError::Internal)?;
        self.get_until(request, deadline)
    }
}

impl ReqwestManifestHttpTransport {
    fn get_until(
        &self,
        request: ManifestHttpRequest,
        deadline: Instant,
    ) -> Result<ManifestHttpResponse, ManifestHttpError> {
        let mut current_url = Url::parse(request.url).map_err(|_| ManifestHttpError::Internal)?;
        if !is_safe_https_url(&current_url)
            || current_url.query().is_some()
            || current_url.fragment().is_some()
        {
            return Err(ManifestHttpError::Internal);
        }
        let addresses =
            validate_public_address_with_deadline(&current_url, self.resolver.as_ref(), deadline)?;
        let host = canonical_host(&current_url).ok_or(ManifestHttpError::Network)?;
        if host.parse::<IpAddr>().is_err() {
            self.pinned_resolver
                .pin(&host, &addresses)
                .map_err(|_| ManifestHttpError::Network)?;
        }
        let mut visited = HashSet::new();
        visited.insert(current_url.to_string());
        let mut redirects = Vec::new();

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ManifestHttpError::Timeout);
            }

            let response = self
                .client
                .get(current_url.clone())
                .timeout(remaining)
                .send()
                .map_err(|error| map_reqwest_error(error, deadline))?;
            let status = response.status().as_u16();

            if is_redirect_status(status) {
                if redirects.len() >= request.max_redirects {
                    return Err(ManifestHttpError::Redirect);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(ManifestHttpError::Redirect)?;
                let next_url = resolve_redirect(&current_url, location)
                    .map_err(|_| ManifestHttpError::Redirect)?;
                let addresses = validate_public_address_with_deadline(
                    &next_url,
                    self.resolver.as_ref(),
                    deadline,
                )?;
                let host = canonical_host(&next_url).ok_or(ManifestHttpError::Network)?;
                if host.parse::<IpAddr>().is_err() {
                    self.pinned_resolver
                        .pin(&host, &addresses)
                        .map_err(|_| ManifestHttpError::Network)?;
                }
                if !visited.insert(next_url.to_string()) {
                    return Err(ManifestHttpError::Redirect);
                }
                redirects.push(ManifestRedirect {
                    status,
                    location: Some(location.to_string()),
                });
                current_url = next_url;
                continue;
            }

            let content_type = response_content_type(&response);
            let content_length = response.content_length();
            if status == 200 {
                if !is_allowed_content_type(content_type.as_deref(), request.accepted_content_types)
                {
                    return Err(ManifestHttpError::ContentType);
                }
                if content_length.is_some_and(|length| length > request.max_body_bytes as u64) {
                    return Err(ManifestHttpError::ResponseTooLarge);
                }
            }

            let body = if status == 200 {
                read_limited_body(response, request.max_body_bytes, deadline)?
            } else {
                Vec::new()
            };

            return Ok(ManifestHttpResponse {
                status,
                content_type,
                content_length,
                body,
                redirects,
                final_url: current_url.to_string(),
            });
        }
    }
}

pub(crate) fn fetch_manifest_from_github() -> Result<UpdateManifest, ManifestProviderError> {
    let transport = ReqwestManifestHttpTransport::new()?;
    fetch_manifest(&transport)
}

pub(crate) fn fetch_manifest<T: ManifestHttpTransport + ?Sized>(
    transport: &T,
) -> Result<UpdateManifest, ManifestProviderError> {
    let request = ManifestHttpRequest::fixed();
    let response = transport
        .get(request.clone())
        .map_err(ManifestProviderError::from)?;

    validate_redirect_trace(transport, &request, &response)?;
    if response.status != 200 {
        return Err(ManifestProviderError::HttpStatus);
    }
    if !is_allowed_content_type(
        response.content_type.as_deref(),
        request.accepted_content_types,
    ) {
        return Err(ManifestProviderError::ContentType);
    }
    if response
        .content_length
        .is_some_and(|length| length > request.max_body_bytes as u64)
        || response.body.len() > request.max_body_bytes
    {
        return Err(ManifestProviderError::ResponseTooLarge);
    }
    if response.body.is_empty() {
        return Err(ManifestProviderError::EmptyResponse);
    }

    let text =
        String::from_utf8(response.body).map_err(|_| ManifestProviderError::InvalidEncoding)?;
    serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|_| ManifestProviderError::InvalidJson)?;
    parse_manifest(&text).map_err(|_| ManifestProviderError::InvalidManifest)
}

fn is_fixed_request(request: &ManifestHttpRequest) -> bool {
    request.url == GITHUB_RELEASES_MANIFEST_URL
        && request.timeout == MANIFEST_FETCH_TIMEOUT
        && request.max_redirects == MAX_MANIFEST_REDIRECT_HOPS
        && request.max_body_bytes == MAX_MANIFEST_BODY_BYTES
        && request.accepted_content_types == ALLOWED_MANIFEST_CONTENT_TYPES
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn is_safe_https_url(url: &Url) -> bool {
    url.scheme().eq_ignore_ascii_case("https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn resolve_redirect(current_url: &Url, location: &str) -> Result<Url, ()> {
    if location.is_empty() || location.trim() != location || location.chars().any(char::is_control)
    {
        return Err(());
    }
    let next_url = current_url.join(location).map_err(|_| ())?;
    if !is_safe_https_url(&next_url) {
        return Err(());
    }
    Ok(next_url)
}

fn validate_redirect_trace<T: ManifestHttpTransport + ?Sized>(
    transport: &T,
    request: &ManifestHttpRequest,
    response: &ManifestHttpResponse,
) -> Result<(), ManifestProviderError> {
    let mut current_url = Url::parse(request.url).map_err(|_| ManifestProviderError::Internal)?;
    if !is_safe_https_url(&current_url)
        || current_url.query().is_some()
        || current_url.fragment().is_some()
    {
        return Err(ManifestProviderError::Internal);
    }
    transport
        .validate_url(&current_url)
        .map_err(ManifestProviderError::from)?;

    if response.redirects.len() > request.max_redirects {
        return Err(ManifestProviderError::Redirect);
    }
    let mut visited = HashSet::new();
    visited.insert(current_url.to_string());
    for redirect in &response.redirects {
        if !is_redirect_status(redirect.status) {
            return Err(ManifestProviderError::Redirect);
        }
        let location = redirect
            .location
            .as_deref()
            .ok_or(ManifestProviderError::Redirect)?;
        let next_url = resolve_redirect(&current_url, location)
            .map_err(|_| ManifestProviderError::Redirect)?;
        transport
            .validate_url(&next_url)
            .map_err(ManifestProviderError::from)?;
        if !visited.insert(next_url.to_string()) {
            return Err(ManifestProviderError::Redirect);
        }
        current_url = next_url;
    }

    let final_url = Url::parse(&response.final_url).map_err(|_| ManifestProviderError::Redirect)?;
    if !is_safe_https_url(&final_url) || final_url != current_url {
        return Err(ManifestProviderError::Redirect);
    }
    transport
        .validate_url(&final_url)
        .map_err(ManifestProviderError::from)?;
    Ok(())
}

fn response_content_type(response: &Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn is_allowed_content_type(content_type: Option<&str>, allowed: &[&str]) -> bool {
    let Some(media_type) = content_type.and_then(|value| value.split(';').next()) else {
        return false;
    };
    let media_type = media_type.trim();
    !media_type.is_empty()
        && allowed
            .iter()
            .any(|candidate| media_type.eq_ignore_ascii_case(candidate))
}

fn read_limited_body<R: Read>(
    mut reader: R,
    max_body_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, ManifestHttpError> {
    let mut body = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(ManifestHttpError::Timeout);
        }
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|error| map_io_error(error, deadline))?;
        if bytes_read == 0 {
            return Ok(body);
        }
        if bytes_read > max_body_bytes.saturating_sub(body.len()) {
            return Err(ManifestHttpError::ResponseTooLarge);
        }
        body.extend_from_slice(&buffer[..bytes_read]);
    }
}

fn map_reqwest_error(error: reqwest::Error, deadline: Instant) -> ManifestHttpError {
    if error.is_timeout() || Instant::now() >= deadline {
        ManifestHttpError::Timeout
    } else {
        ManifestHttpError::Network
    }
}

fn map_io_error(error: io::Error, deadline: Instant) -> ManifestHttpError {
    if error.kind() == io::ErrorKind::TimedOut || Instant::now() >= deadline {
        ManifestHttpError::Timeout
    } else {
        ManifestHttpError::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::net::SocketAddr;
    use std::sync::Arc;

    const VALID_MANIFEST: &[u8] = br#"{
      "productId":"com.cornellmethod.notebook",
      "schemaVersion":1,
      "releases":[{
        "channel":"stable",
        "version":"1.2.3",
        "architecture":"aarch64-apple-darwin",
        "minVersion":"14",
        "artifact":{
          "artifactId":"cornell-method-notebook-1.2.3",
          "format":"app-archive",
          "url":"https://updates.example.test/cornell-method-notebook-1.2.3",
          "sizeBytes":123,
          "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "signature":{"keyId":"test-key","proof":"opaque-proof"}
      }]
    }"#;

    fn response(body: impl Into<Vec<u8>>) -> ManifestHttpResponse {
        let body = body.into();
        ManifestHttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            content_length: Some(body.len() as u64),
            body,
            redirects: Vec::new(),
            final_url: GITHUB_RELEASES_MANIFEST_URL.to_string(),
        }
    }

    fn response_with_content_type(
        content_type: Option<&str>,
        body: impl Into<Vec<u8>>,
    ) -> ManifestHttpResponse {
        let mut result = response(body);
        result.content_type = content_type.map(str::to_owned);
        result
    }

    fn response_with_redirects(
        redirects: Vec<(u16, Option<&str>)>,
        final_url: &str,
    ) -> ManifestHttpResponse {
        let mut result = response(VALID_MANIFEST);
        result.redirects = redirects
            .into_iter()
            .map(|(status, location)| ManifestRedirect {
                status,
                location: location.map(str::to_owned),
            })
            .collect();
        result.final_url = final_url.to_string();
        result
    }

    struct FakeTransport {
        result: Result<ManifestHttpResponse, ManifestHttpError>,
        requests: RefCell<Vec<ManifestHttpRequest>>,
    }

    impl FakeTransport {
        fn response(response: ManifestHttpResponse) -> Self {
            Self {
                result: Ok(response),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn error(error: ManifestHttpError) -> Self {
            Self {
                result: Err(error),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl ManifestHttpTransport for FakeTransport {
        fn get(
            &self,
            request: ManifestHttpRequest,
        ) -> Result<ManifestHttpResponse, ManifestHttpError> {
            self.requests.borrow_mut().push(request);
            self.result.clone()
        }
    }

    #[derive(Default)]
    struct FakeAddressResolver {
        results: HashMap<String, Result<Vec<SocketAddr>, ()>>,
    }

    impl FakeAddressResolver {
        fn addresses(mut self, host: &str, addresses: &[&str]) -> Self {
            self.results.insert(
                host.to_ascii_lowercase(),
                Ok(addresses
                    .iter()
                    .map(|address| address.parse().unwrap())
                    .collect()),
            );
            self
        }

        fn failure(mut self, host: &str) -> Self {
            self.results.insert(host.to_ascii_lowercase(), Err(()));
            self
        }
    }

    impl PublicAddressResolver for FakeAddressResolver {
        fn resolve(&self, host: &str, _port: u16) -> Result<Vec<SocketAddr>, ()> {
            self.results
                .get(&host.to_ascii_lowercase())
                .cloned()
                .unwrap_or(Err(()))
        }
    }

    struct DelayedAddressResolver {
        delay: Duration,
        addresses: Vec<SocketAddr>,
    }

    impl PublicAddressResolver for DelayedAddressResolver {
        fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>, ()> {
            std::thread::sleep(self.delay);
            Ok(self.addresses.clone())
        }

        fn resolve_with_deadline(
            &self,
            _host: &str,
            _port: u16,
            deadline: Instant,
        ) -> Result<Vec<SocketAddr>, PublicAddressResolveError> {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining < self.delay {
                std::thread::sleep(remaining);
                return Err(PublicAddressResolveError::Timeout);
            }
            std::thread::sleep(self.delay);
            Ok(self.addresses.clone())
        }
    }

    struct ResolverTransport {
        response: ManifestHttpResponse,
        resolver: FakeAddressResolver,
    }

    impl ManifestHttpTransport for ResolverTransport {
        fn get(
            &self,
            _request: ManifestHttpRequest,
        ) -> Result<ManifestHttpResponse, ManifestHttpError> {
            Ok(self.response.clone())
        }

        fn validate_url(&self, url: &Url) -> Result<(), ManifestHttpError> {
            validate_public_address(url, &self.resolver)
                .map(|_| ())
                .map_err(|_| ManifestHttpError::Network)
        }
    }

    fn assert_provider_error<T>(result: Result<T, ManifestProviderError>, code: &str) {
        let error = match result {
            Ok(_) => panic!("expected provider failure"),
            Err(error) => error,
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.to_string(), code);
    }

    #[test]
    fn rejects_loopback_private_link_local_multicast_and_reserved_ip_literals() {
        for url in [
            "https://127.0.0.1/manifest",
            "https://10.0.0.1/manifest",
            "https://172.16.0.1/manifest",
            "https://192.168.1.1/manifest",
            "https://169.254.1.1/manifest",
            "https://0.0.0.0/manifest",
            "https://224.0.0.1/manifest",
            "https://192.0.2.1/manifest",
            "https://[::1]/manifest",
            "https://[::]/manifest",
            "https://[fc00::1]/manifest",
            "https://[fe80::1]/manifest",
            "https://[ff02::1]/manifest",
            "https://[2001:db8::1]/manifest",
            "https://[::ffff:192.168.1.1]/manifest",
        ] {
            let url = Url::parse(url).unwrap();
            assert_eq!(
                validate_public_address(&url, &FakeAddressResolver::default()),
                Err(PublicAddressError::Unsafe),
                "address should be rejected: {url}"
            );
        }
    }

    #[test]
    fn classifies_dns_failure_private_resolution_and_mixed_candidates_without_details() {
        let private_url = Url::parse("https://private.example.test/manifest").unwrap();
        let mixed_url = Url::parse("https://mixed.example.test/manifest").unwrap();
        let resolver = FakeAddressResolver::default()
            .addresses("private.example.test", &["10.0.0.1:443"])
            .addresses(
                "mixed.example.test",
                &["93.184.216.34:443", "192.168.1.1:443"],
            );

        assert_eq!(
            validate_public_address(&private_url, &resolver),
            Err(PublicAddressError::Unsafe)
        );
        assert_eq!(
            validate_public_address(&mixed_url, &resolver),
            Err(PublicAddressError::Mixed)
        );
        assert_eq!(
            validate_public_address(
                &Url::parse("https://missing.example.test/manifest").unwrap(),
                &resolver.failure("missing.example.test")
            ),
            Err(PublicAddressError::Resolve)
        );
    }

    #[test]
    fn accepts_public_https_ipv4_and_ipv6_candidates() {
        let resolver = FakeAddressResolver::default().addresses(
            "public.example.test",
            &["93.184.216.34:443", "[2001:4860:4860::8888]:443"],
        );
        let url = Url::parse("https://public.example.test/manifest").unwrap();
        assert!(validate_public_address(&url, &resolver).is_ok());
    }

    #[test]
    fn reqwest_provider_rejects_dns_failure_before_request() {
        let resolver = FakeAddressResolver::default().failure("github.com");
        let transport = ReqwestManifestHttpTransport::with_resolver(Arc::new(resolver)).unwrap();
        assert_eq!(
            transport.get(ManifestHttpRequest::fixed()),
            Err(ManifestHttpError::Network)
        );
    }

    #[test]
    fn delayed_dns_resolution_returns_timeout_before_http_request() {
        let resolver = DelayedAddressResolver {
            delay: Duration::from_millis(100),
            addresses: vec!["93.184.216.34:443".parse().unwrap()],
        };
        let transport = ReqwestManifestHttpTransport::with_resolver(Arc::new(resolver)).unwrap();
        let deadline = Instant::now() + Duration::from_millis(10);
        let started = Instant::now();

        assert_eq!(
            transport.get_until(ManifestHttpRequest::fixed(), deadline),
            Err(ManifestHttpError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn rejects_private_and_mixed_redirect_hops_before_manifest_acceptance() {
        for (host, addresses, expected_code) in [
            (
                "private.example.test",
                vec!["10.0.0.1:443"],
                "provider-network",
            ),
            (
                "mixed.example.test",
                vec!["93.184.216.34:443", "192.168.1.1:443"],
                "provider-network",
            ),
        ] {
            let final_url = format!("https://{host}/manifest");
            let response = response_with_redirects(vec![(302, Some(&final_url))], &final_url);
            let resolver = FakeAddressResolver::default()
                .addresses("github.com", &["93.184.216.34:443"])
                .addresses(host, &addresses);
            assert_provider_error(
                fetch_manifest(&ResolverTransport { response, resolver }),
                expected_code,
            );
        }
    }

    #[test]
    fn sends_only_the_fixed_get_request_policy_to_the_transport() {
        let fake = FakeTransport::response(response(VALID_MANIFEST));
        fetch_manifest(&fake).unwrap();

        let requests = fake.requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, GITHUB_RELEASES_MANIFEST_URL);
        assert_eq!(requests[0].timeout, Duration::from_secs(15));
        assert_eq!(requests[0].max_redirects, 5);
        assert_eq!(requests[0].max_body_bytes, 1_048_576);
        assert_eq!(
            requests[0].accepted_content_types,
            &["application/json", "application/octet-stream"]
        );
    }

    #[test]
    fn accepts_json_content_types_case_insensitively_with_parameters() {
        for content_type in [
            Some("application/json"),
            Some("Application/JSON; charset=utf-8"),
            Some("application/octet-stream; version=1"),
        ] {
            let fake =
                FakeTransport::response(response_with_content_type(content_type, VALID_MANIFEST));
            assert!(
                fetch_manifest(&fake).is_ok(),
                "content type: {content_type:?}"
            );
        }
    }

    #[test]
    fn rejects_missing_or_unsupported_content_types_without_sniffing() {
        for content_type in [None, Some("text/html"), Some("application/jsonish")] {
            let fake =
                FakeTransport::response(response_with_content_type(content_type, VALID_MANIFEST));
            assert_provider_error(fetch_manifest(&fake), "provider-content-type");
        }
    }

    #[test]
    fn accepts_only_final_200_status() {
        for status in [204, 206, 304, 301, 404, 500] {
            let mut result = response(VALID_MANIFEST);
            result.status = status;
            let fake = FakeTransport::response(result);
            assert_provider_error(fetch_manifest(&fake), "provider-http-status");
        }
    }

    #[test]
    fn rejects_empty_and_oversized_responses_at_the_boundary() {
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(response(Vec::new()))),
            "provider-empty-response",
        );

        let mut exactly_at_limit = VALID_MANIFEST.to_vec();
        exactly_at_limit.resize(MAX_MANIFEST_BODY_BYTES, b' ');
        assert!(fetch_manifest(&FakeTransport::response(response(exactly_at_limit))).is_ok());

        let mut declared_too_large = response(VALID_MANIFEST);
        declared_too_large.content_length = Some((MAX_MANIFEST_BODY_BYTES + 1) as u64);
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(declared_too_large)),
            "provider-response-too-large",
        );

        let mut body_too_large = VALID_MANIFEST.to_vec();
        body_too_large.resize(MAX_MANIFEST_BODY_BYTES + 1, b' ');
        let mut streamed_too_large = response(body_too_large);
        streamed_too_large.content_length = None;
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(streamed_too_large)),
            "provider-response-too-large",
        );
    }

    #[test]
    fn streaming_reader_stops_when_the_body_cap_is_exceeded() {
        let body = vec![b'x'; MAX_MANIFEST_BODY_BYTES + 1];
        let error = read_limited_body(
            Cursor::new(body),
            MAX_MANIFEST_BODY_BYTES,
            Instant::now() + Duration::from_secs(1),
        )
        .expect_err("streamed body should be rejected");
        assert_eq!(error, ManifestHttpError::ResponseTooLarge);
    }

    #[test]
    fn classifies_transport_timeout_network_and_tls_errors_without_details() {
        for (transport_error, provider_code) in [
            (ManifestHttpError::Timeout, "provider-timeout"),
            (ManifestHttpError::Network, "provider-network"),
            (ManifestHttpError::Tls, "provider-network"),
        ] {
            let fake = FakeTransport::error(transport_error);
            assert_provider_error(fetch_manifest(&fake), provider_code);
        }
    }

    #[test]
    fn accepts_each_allowed_https_redirect_status_and_opaque_query() {
        for status in [301, 302, 303, 307, 308] {
            let final_url =
                "https://release-assets.githubusercontent.com/manifest.json?opaque=provider-value";
            let fake = FakeTransport::response(response_with_redirects(
                vec![(status, Some(final_url))],
                final_url,
            ));
            assert!(fetch_manifest(&fake).is_ok(), "redirect status: {status}");
        }
    }

    #[test]
    fn accepts_at_most_five_redirect_hops_and_rejects_six() {
        let five_hops = vec![
            (301, Some("https://github.com/hop-1")),
            (302, Some("https://github.com/hop-2")),
            (303, Some("https://github.com/hop-3")),
            (307, Some("https://github.com/hop-4")),
            (308, Some("https://github.com/hop-5")),
        ];
        assert!(
            fetch_manifest(&FakeTransport::response(response_with_redirects(
                five_hops,
                "https://github.com/hop-5",
            )))
            .is_ok()
        );

        let six_hops = vec![
            (301, Some("https://github.com/hop-1")),
            (302, Some("https://github.com/hop-2")),
            (303, Some("https://github.com/hop-3")),
            (307, Some("https://github.com/hop-4")),
            (308, Some("https://github.com/hop-5")),
            (301, Some("https://github.com/hop-6")),
        ];
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(response_with_redirects(
                six_hops,
                "https://github.com/hop-6",
            ))),
            "provider-redirect",
        );
    }

    #[test]
    fn rejects_missing_location_loops_downgrades_non_https_and_userinfo() {
        let cases = [
            (vec![(301, None)], GITHUB_RELEASES_MANIFEST_URL),
            (
                vec![(302, Some(GITHUB_RELEASES_MANIFEST_URL))],
                GITHUB_RELEASES_MANIFEST_URL,
            ),
            (
                vec![(303, Some("http://github.com/downgrade"))],
                "http://github.com/downgrade",
            ),
            (
                vec![(307, Some("ftp://github.com/not-https"))],
                "ftp://github.com/not-https",
            ),
            (
                vec![(308, Some("https://user:password@github.com/private"))],
                "https://user:password@github.com/private",
            ),
        ];
        for (redirects, final_url) in cases {
            assert_provider_error(
                fetch_manifest(&FakeTransport::response(response_with_redirects(
                    redirects, final_url,
                ))),
                "provider-redirect",
            );
        }
    }

    #[test]
    fn rejects_invalid_utf8_bom_and_malformed_json() {
        let mut invalid_utf8 = response(vec![0xff, 0xfe]);
        invalid_utf8.content_length = Some(2);
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(invalid_utf8)),
            "provider-invalid-encoding",
        );

        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(VALID_MANIFEST);
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(response(bom))),
            "provider-invalid-json",
        );

        assert_provider_error(
            fetch_manifest(&FakeTransport::response(response(b"{not-json"))),
            "provider-invalid-json",
        );
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(response(b"   \n\t"))),
            "provider-invalid-json",
        );
    }

    #[test]
    fn delegates_schema_and_logical_validation_to_the_existing_manifest_parser() {
        let mut invalid_manifest = response(VALID_MANIFEST);
        invalid_manifest.body = br#"{
          "productId":"com.cornellmethod.notebook",
          "schemaVersion":1,
          "releases":[],
          "unexpected":true
        }"#
        .to_vec();
        invalid_manifest.content_length = Some(invalid_manifest.body.len() as u64);
        assert_provider_error(
            fetch_manifest(&FakeTransport::response(invalid_manifest)),
            "provider-invalid-manifest",
        );

        let empty =
            br#"{"productId":"com.cornellmethod.notebook","schemaVersion":1,"releases":[]}"#;
        let result = fetch_manifest(&FakeTransport::response(response(empty))).unwrap();
        assert!(result.is_no_update());

        let non_target = br#"{
          "productId":"com.cornellmethod.notebook",
          "schemaVersion":1,
          "releases":[{
            "channel":"beta",
            "version":"9.0.0",
            "architecture":"x86_64-apple-darwin",
            "minVersion":"14",
            "artifact":{
              "artifactId":"non-target",
              "format":"app-archive",
              "url":"https://updates.example.test/non-target",
              "sizeBytes":1,
              "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            },
            "signature":{"keyId":"test-key","proof":"opaque-proof"}
          }]
        }"#;
        let result = fetch_manifest(&FakeTransport::response(response(non_target))).unwrap();
        assert!(result.is_no_update());
    }

    #[test]
    fn provider_errors_do_not_carry_response_body_or_url_details() {
        let mut invalid_manifest = response(VALID_MANIFEST);
        invalid_manifest.body = br#"{"secret":"response-body"}"#.to_vec();
        let error = fetch_manifest(&FakeTransport::response(invalid_manifest)).unwrap_err();
        assert_eq!(error, ManifestProviderError::InvalidManifest);
        assert_eq!(error.to_string(), "provider-invalid-manifest");
        assert!(!format!("{error:?}").contains("response-body"));
        assert!(!format!("{error:?}").contains(GITHUB_RELEASES_MANIFEST_URL));
    }
}
