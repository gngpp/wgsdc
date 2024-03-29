use libc::c_char;

use crate::{backends, key::Key, Backend, KeyPair, PeerConfigBuilder};

#[cfg(feature = "print")]
use colored::Colorize;

use std::{
    borrow::Cow,
    ffi::CStr,
    fmt, io,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::SystemTime,
};

/// Represents an IP address a peer is allowed to have, in CIDR notation.
#[derive(PartialEq, Eq, Clone)]
pub struct AllowedIp {
    /// The IP address.
    pub address: IpAddr,
    /// The CIDR subnet mask.
    pub cidr: u8,
}

impl AllowedIp {
    pub fn new(address: IpAddr, cidr: u8) -> Self {
        Self { address, cidr }
    }
}

impl fmt::Debug for AllowedIp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.address, self.cidr)
    }
}

impl FromStr for AllowedIp {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = s.split('/').collect();
        if parts.len() != 2 {
            return Err(());
        }

        Ok(AllowedIp {
            address: parts[0].parse().map_err(|_| ())?,
            cidr: parts[1].parse().map_err(|_| ())?,
        })
    }
}

/// Represents a single peer's configuration (i.e. persistent attributes).
///
/// These are the attributes that don't change over time and are part of the configuration.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PeerConfig {
    /// The public key of the peer.
    pub public_key: Key,
    /// The preshared key available to both peers (`None` means no PSK is used).
    pub preshared_key: Option<Key>,
    /// The endpoint this peer listens for connections on (`None` means any).
    pub endpoint: Option<SocketAddr>,
    /// The interval for sending keepalive packets (`None` means disabled).
    pub persistent_keepalive_interval: Option<u16>,
    /// The IP addresses this peer is allowed to have.
    pub allowed_ips: Vec<AllowedIp>,
    pub(crate) __cant_construct_me: (),
}

/// Represents a single peer's current statistics (i.e. the data from the current session).
///
/// These are the attributes that will change over time; to update them,
/// re-read the information from the interface.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct PeerStats {
    /// Time of the last handshake/rekey with this peer.
    pub last_handshake_time: Option<SystemTime>,
    /// Number of bytes received from this peer.
    pub rx_bytes: u64,
    /// Number of bytes transmitted to this peer.
    pub tx_bytes: u64,
}

/// Represents the complete status of a peer.
///
/// This struct simply combines [`PeerInfo`](PeerInfo) and [`PeerStats`](PeerStats)
/// to represent all available information about a peer.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PeerInfo {
    pub config: PeerConfig,
    pub stats: PeerStats,
}

/// Represents all available information about a WireGuard device (interface).
///
/// This struct contains the current configuration of the device
/// and the current configuration _and_ state of all of its peers.
/// The peer statistics are retrieved once at construction time,
/// and need to be updated manually by calling [`get_by_name`](DeviceInfo::get_by_name).
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Device {
    /// The interface name of this device
    pub name: InterfaceName,
    /// The public encryption key of this interface (if present)
    pub public_key: Option<Key>,
    /// The private encryption key of this interface (if present)
    pub private_key: Option<Key>,
    /// The [fwmark](https://www.linux.org/docs/man8/tc-fw.html) of this interface
    pub fwmark: Option<u32>,
    /// The port to listen for incoming connections on
    pub listen_port: Option<u16>,
    /// The list of all registered peers and their information
    pub peers: Vec<PeerInfo>,
    /// The associated "real name" of the interface (ex. "utun8" on macOS).
    pub linked_name: Option<String>,
    /// The backend the device exists on (userspace or kernel).
    pub backend: Backend,

    pub(crate) __cant_construct_me: (),
}

type RawInterfaceName = [c_char; libc::IFNAMSIZ];

/// The name of a Wireguard interface device.
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct InterfaceName(RawInterfaceName);

impl FromStr for InterfaceName {
    type Err = InvalidInterfaceName;

    /// Attempts to parse a Rust string as a valid Linux interface name.
    ///
    /// Extra validation logic ported from [iproute2](https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/tree/lib/utils.c#n827)
    fn from_str(name: &str) -> Result<Self, InvalidInterfaceName> {
        let len = name.len();
        if len == 0 {
            return Err(InvalidInterfaceName::Empty);
        }

        // Ensure its short enough to include a trailing NUL
        if len > (libc::IFNAMSIZ - 1) {
            return Err(InvalidInterfaceName::TooLong);
        }

        let mut buf = [c_char::default(); libc::IFNAMSIZ];
        // Check for interior NULs and other invalid characters.
        for (out, b) in buf.iter_mut().zip(name.as_bytes().iter()) {
            if *b == 0 || *b == b'/' || b.is_ascii_whitespace() {
                return Err(InvalidInterfaceName::InvalidChars);
            }

            *out = *b as c_char;
        }
        Ok(Self(buf))
    }
}

impl InterfaceName {
    /// Returns a human-readable form of the device name.
    ///
    /// Only use this when the interface name was constructed from a Rust string.
    pub fn as_str_lossy(&self) -> Cow<'_, str> {
        // SAFETY: These are C strings coming from wgctrl, so they are correctly NUL terminated.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }.to_string_lossy()
    }

    #[cfg(target_os = "linux")]
    /// Returns a pointer to the inner byte buffer for FFI calls.
    pub fn as_ptr(&self) -> *const c_char {
        self.0.as_ptr()
    }
}

impl fmt::Debug for InterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str_lossy())
    }
}

impl fmt::Display for InterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str_lossy())
    }
}

/// An interface name was bad.
#[derive(Debug, PartialEq, Eq)]
pub enum InvalidInterfaceName {
    /// Provided name was longer then the interface name length limit
    /// of the system.
    TooLong,

    // These checks are done in the kernel as well, but no reason to let bad names
    // get that far: https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/tree/lib/utils.c?id=1f420318bda3cc62156e89e1b56d60cc744b48ad#n827.
    /// Interface name was an empty string.
    Empty,
    /// Interface name contained a nul, `/` or whitespace character.
    InvalidChars,
}

impl fmt::Display for InvalidInterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(
                f,
                "interface name longer than system max of {} chars",
                libc::IFNAMSIZ
            ),
            Self::Empty => f.write_str("an empty interface name was provided"),
            Self::InvalidChars => f.write_str("interface name contained slash or space characters"),
        }
    }
}

impl From<InvalidInterfaceName> for io::Error {
    fn from(e: InvalidInterfaceName) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, e.to_string())
    }
}

impl std::error::Error for InvalidInterfaceName {}

impl Device {
    /// Enumerates all WireGuard interfaces currently present in the system,
    /// both with kernel and userspace backends.
    ///
    /// You can use [`get_by_name`](DeviceInfo::get_by_name) to retrieve more
    /// detailed information on each interface.
    pub fn list(backend: Backend) -> Result<Vec<InterfaceName>, io::Error> {
        match backend {
            #[cfg(target_os = "linux")]
            Backend::Kernel => backends::kernel::enumerate(),
            Backend::Userspace => backends::userspace::enumerate(),
        }
    }

    pub fn get(name: &InterfaceName, backend: Backend) -> Result<Self, io::Error> {
        match backend {
            #[cfg(target_os = "linux")]
            Backend::Kernel => backends::kernel::get_by_name(name),
            Backend::Userspace => backends::userspace::get_by_name(name),
        }
    }

    #[cfg(feature = "print")]
    pub fn print(&self) -> Result<(), std::time::SystemTimeError> {
        println!(
            "{}: {}",
            "interface".green(),
            self.name.as_str_lossy().green()
        );
        if let Some(public_key) = &self.public_key {
            println!(
                "  {}: {}",
                "public key".white().bold(),
                public_key.to_base64()
            );
        }

        if let Some(_private_key) = &self.public_key {
            println!("  {}: {}", "private key".white().bold(), "(hidden)");
        }

        if let Some(listen_port) = self.listen_port {
            println!("  {}: {}", "listen port".white().bold(), listen_port);
        }

        for peer in &self.peers {
            println!();
            Self::print_peer(peer)?;
        }

        Ok(())
    }

    #[cfg(feature = "print")]
    fn print_peer(peer: &PeerInfo) -> Result<(), std::time::SystemTimeError> {
        println!(
            "{}: {}",
            "peer".yellow(),
            peer.config.public_key.to_base64().as_str().yellow()
        );

        if let Some(_preshare_key) = &peer.config.preshared_key {
            println!("  {}: {}", "preshared key".white().bold(), "(hidden)");
        }
        if let Some(endpoint) = peer.config.endpoint {
            println!("  {}: {}", "endpoint".white().bold(), endpoint);
        }

        if !peer.config.allowed_ips.is_empty() {
            print!("  {}: ", "allowed ips".white().bold());
            for (i, allowed_ip) in peer.config.allowed_ips.iter().enumerate() {
                print!("{}{}{}", allowed_ip.address, "/".cyan(), allowed_ip.cidr);
                if i < peer.config.allowed_ips.len() - 1 {
                    print!(", ");
                } else {
                    println!()
                }
            }
        }

        if let Some(keepalive) = peer.config.persistent_keepalive_interval {
            if keepalive > 0 {
                println!(
                    "  {}: every {} {}",
                    "persistent keepalive".white().bold(),
                    keepalive,
                    "seconds".cyan()
                )
            }
        }

        if let Some(latest_handshake) = &peer.stats.last_handshake_time {
            // latest handshake may be 0 on Linux devices
            let timestamp = latest_handshake
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("Time went backwards");
            if timestamp.as_secs() > 0 {
                println!(
                    "  {}: {}",
                    "latest handshake".white().bold(),
                    Self::calculate_time(latest_handshake)?
                );
            }
        }

        if peer.stats.tx_bytes > 0 || peer.stats.rx_bytes > 0 {
            use byte_unit::Byte;

            let rx_byte = Byte::from(peer.stats.rx_bytes).get_appropriate_unit(true);

            let tx_byte = Byte::from(peer.stats.tx_bytes).get_appropriate_unit(true);
            println!(
                "  {}: {} {}, {} {}",
                "transfer".white().bold(),
                format!(
                    "{:.2} {}",
                    rx_byte.get_value(),
                    rx_byte.get_unit().to_string().cyan()
                ),
                "received",
                format!(
                    "{:.2} {}",
                    tx_byte.get_value(),
                    tx_byte.get_unit().to_string().cyan()
                ),
                "sent"
            );
        }
        Ok(())
    }

    #[cfg(feature = "print")]
    fn calculate_time(latest_handshake: &SystemTime) -> Result<String, std::time::SystemTimeError> {
        // Convert 100000 seconds to specific year, month, day, hour, minute, second
        let mut seconds = SystemTime::now()
            .duration_since(*latest_handshake)?
            .as_secs();
        let years: u64;
        let months: u64;
        let days: u64;
        let hours: u64;
        let minutes: u64;
        // Calculate the number of years used
        years = seconds / 31_536_000; // 365 * 86400
        seconds %= 31_536_000;

        // Calculate the number of months used
        months = seconds / 2_628_000; // 30 * 86400
        seconds %= 2_628_000;

        // Calculate the number of days used
        days = seconds / 86_400; // 86400
        seconds %= 86_400;

        // Calculate hours used
        hours = seconds / 3_600; // 3600
        seconds %= 3_600;

        // Calculate the number of minutes used
        minutes = seconds / 60;
        seconds %= 60;

        let mut format_time = String::new();
        if years > 0 {
            format_time.push_str(&format!(" {} {},", years, "years".cyan()));
        }

        if months > 0 {
            format_time.push_str(&format!(" {} {},", months, "months".cyan()));
        }

        if days > 0 {
            format_time.push_str(&format!(" {} {},", days, "days".cyan()));
        }

        if hours > 0 {
            format_time.push_str(&format!(" {} {},", hours, "hours".cyan()));
        }

        if minutes > 0 {
            format_time.push_str(&format!(" {} {},", minutes, "minutes".cyan()));
        }

        if seconds > 0 {
            format_time.push_str(&format!(" {} {}", seconds, "seconds ago".cyan()));
        }

        Ok(format_time)
    }

    pub fn delete(self) -> io::Result<()> {
        match self.backend {
            #[cfg(target_os = "linux")]
            Backend::Kernel => backends::kernel::delete_interface(&self.name),
            Backend::Userspace => backends::userspace::delete_interface(&self.name),
        }
    }
}

/// Builds and represents a configuration that can be applied to a WireGuard interface.
///
/// This is the primary way of changing the settings of an interface.
///
/// Note that if an interface exists, the configuration is applied _on top_ of the existing
/// settings, and missing parts are not overwritten or set to defaults.
///
/// If this is not what you want, use [`delete_interface`](delete_interface)
/// to remove the interface entirely before applying the new configuration.
///
/// # Example
/// ```rust
/// use wg::*;
/// use std::net::AddrParseError;
/// fn try_main() -> Result<(), AddrParseError> {
/// let our_keypair = KeyPair::generate();
/// let peer_keypair = KeyPair::generate();
/// let server_addr = "192.168.1.1:51820".parse()?;
///
/// DeviceUpdate::new()
///     .set_keypair(our_keypair)
///     .replace_peers()
///     .add_peer_with(&peer_keypair.public, |peer| {
///         peer.set_endpoint(server_addr)
///             .replace_allowed_ips()
///             .allow_all_ips()
///     }).apply(&"wg-examples".parse().unwrap(), Backend::Userspace).expect("apply device error");
///
/// println!("Send these keys to your peer: {:#?}", peer_keypair);
///
/// Ok(())
/// }
/// fn main() -> Result<(), AddrParseError> { try_main() }
/// ```
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DeviceUpdate {
    pub(crate) public_key: Option<Key>,
    pub(crate) private_key: Option<Key>,
    pub(crate) fwmark: Option<u32>,
    pub(crate) listen_port: Option<u16>,
    pub(crate) peers: Vec<PeerConfigBuilder>,
    pub(crate) replace_peers: bool,
}

impl DeviceUpdate {
    /// Creates a new `DeviceConfigBuilder` that does nothing when applied.
    #[must_use]
    pub fn new() -> Self {
        DeviceUpdate {
            public_key: None,
            private_key: None,
            fwmark: None,
            listen_port: None,
            peers: vec![],
            replace_peers: false,
        }
    }

    /// Sets a new keypair to be applied to the interface.
    ///
    /// This is a convenience method that simply wraps
    /// [`set_public_key`](DeviceConfigBuilder::set_public_key)
    /// and [`set_private_key`](DeviceConfigBuilder::set_private_key).
    #[must_use]
    pub fn set_keypair(self, keypair: KeyPair) -> Self {
        self.set_public_key(keypair.public)
            .set_private_key(keypair.private)
    }

    /// Specifies a new public key to be applied to the interface.
    #[must_use]
    pub fn set_public_key(mut self, key: Key) -> Self {
        self.public_key = Some(key);
        self
    }

    /// Specifies that the public key for this interface should be unset.
    #[must_use]
    pub fn unset_public_key(self) -> Self {
        self.set_public_key(Key::zero())
    }

    /// Sets a new private key to be applied to the interface.
    #[must_use]
    pub fn set_private_key(mut self, key: Key) -> Self {
        self.private_key = Some(key);
        self
    }

    /// Specifies that the private key for this interface should be unset.
    #[must_use]
    pub fn unset_private_key(self) -> Self {
        self.set_private_key(Key::zero())
    }

    /// Specifies the fwmark value that should be applied to packets coming from the interface.
    #[must_use]
    pub fn set_fwmark(mut self, fwmark: u32) -> Self {
        self.fwmark = Some(fwmark);
        self
    }

    /// Specifies that fwmark should not be set on packets from the interface.
    #[must_use]
    pub fn unset_fwmark(self) -> Self {
        self.set_fwmark(0)
    }

    /// Specifies the port to listen for incoming packets on.
    ///
    /// This is useful for a server configuration that listens on a fixed endpoint.
    #[must_use]
    pub fn set_listen_port(mut self, port: u16) -> Self {
        self.listen_port = Some(port);
        self
    }

    /// Specifies that a random port should be used for incoming packets.
    ///
    /// This is probably what you want in client configurations.
    #[must_use]
    pub fn randomize_listen_port(self) -> Self {
        self.set_listen_port(0)
    }

    /// Specifies a new peer configuration to be added to the interface.
    ///
    /// See [`PeerConfigBuilder`](PeerConfigBuilder) for details on building
    /// peer configurations. This method can be called more than once, and all
    /// peers will be added to the configuration.
    #[must_use]
    pub fn add_peer(mut self, peer: PeerConfigBuilder) -> Self {
        self.peers.push(peer);
        self
    }

    /// Specifies a new peer configuration using a builder function.
    ///
    /// This is simply a convenience method to make adding peers more fluent.
    /// This method can be called more than once, and all peers will be added
    /// to the configuration.
    #[must_use]
    pub fn add_peer_with(
        self,
        pubkey: &Key,
        builder: impl Fn(PeerConfigBuilder) -> PeerConfigBuilder,
    ) -> Self {
        self.add_peer(builder(PeerConfigBuilder::new(pubkey)))
    }

    /// Specifies multiple peer configurations to be added to the interface.
    #[must_use]
    pub fn add_peers(mut self, peers: &[PeerConfigBuilder]) -> Self {
        self.peers.extend_from_slice(peers);
        self
    }

    /// Specifies that the peer configurations in this `DeviceConfigBuilder` should
    /// replace the existing configurations on the interface, not modify or append to them.
    #[must_use]
    pub fn replace_peers(mut self) -> Self {
        self.replace_peers = true;
        self
    }

    /// Specifies that the peer with this public key should be removed from the interface.
    #[must_use]
    pub fn remove_peer_by_key(self, public_key: &Key) -> Self {
        let mut peer = PeerConfigBuilder::new(public_key);
        peer.remove_me = true;
        self.add_peer(peer)
    }

    /// Build and apply the configuration to a WireGuard interface by name.
    ///
    /// An interface with the provided name will be created if one does not exist already.
    pub fn apply(self, iface: &InterfaceName, backend: Backend) -> io::Result<()> {
        match backend {
            #[cfg(target_os = "linux")]
            Backend::Kernel => backends::kernel::apply(&self, iface),
            Backend::Userspace => backends::userspace::apply(&self, iface),
        }
    }
}

impl Default for DeviceUpdate {
    fn default() -> Self {
        Self::new()
    }
}
