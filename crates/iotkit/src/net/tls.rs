//! TLS 1.3 for MQTT (port 8883) and `https://` downloads (`tls` feature), with
//! [embedded-tls](https://docs.rs/embedded-tls): AES-128-GCM, the server's certificate checked
//! against one CA built in from `IOTKIT_TLS_CA` (a DER file, ECDSA P-256), and its name against
//! the host the device connects to. There is no clock yet: certificates are not checked for
//! expiry.

use embassy_net::tcp::TcpSocket;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embedded_tls::{
    Aes128GcmSha256, Certificate, CryptoProvider, NoClock, TlsConfig, TlsConnection, TlsContext, TlsError, TlsVerifier,
    pki::CertVerifier,
};
use esp_hal::rng::Rng;

/// The CA the servers' certificates must chain to.
static CA: &[u8] = include_bytes!(env!(
    "IOTKIT_TLS_CA",
    "the `tls` feature needs IOTKIT_TLS_CA: the path of the CA certificate (DER) the servers' certificates chain to"
));

/// A whole TLS record: what servers send when the client cannot ask for less.
pub const READ_BUFFER: usize = 16_640;
/// Records the device sends (an MQTT packet or a GET request fits).
pub const WRITE_BUFFER: usize = 4096;

/// A TLS connection over a TCP socket.
pub type Connection<'a> = TlsConnection<'a, TcpSocket<'a>, Aes128GcmSha256>;

/// One connection's record buffers. In a static: an array this size built in an `async fn`
/// would pass through the stack first.
pub struct Buffers {
    /// Records from the server.
    pub read: [u8; READ_BUFFER],
    /// Records to the server.
    pub write: [u8; WRITE_BUFFER],
}

/// [`Buffers`] for one user at a time.
pub type SharedBuffers = Mutex<CriticalSectionRawMutex, Buffers>;

impl Buffers {
    /// For a `static`.
    pub const fn shared() -> SharedBuffers {
        Mutex::new(Self { read: [0; READ_BUFFER], write: [0; WRITE_BUFFER] })
    }
}

/// The hardware RNG: truly random while the radio runs, as on any Wi-Fi device.
struct Entropy(Rng);

impl rand_core::RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        rand_core::RngCore::next_u32(&mut self.0)
    }
    fn next_u64(&mut self) -> u64 {
        rand_core::RngCore::next_u64(&mut self.0)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        rand_core::RngCore::fill_bytes(&mut self.0, dest)
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        rand_core::RngCore::try_fill_bytes(&mut self.0, dest)
    }
}

impl rand_core::CryptoRng for Entropy {}

struct Provider {
    rng: Entropy,
    verifier: CertVerifier<'static, Aes128GcmSha256, NoClock, 2048>,
}

impl CryptoProvider for Provider {
    type CipherSuite = Aes128GcmSha256;
    type Signature = [u8; 0];

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Aes128GcmSha256>, TlsError> {
        Ok(&mut self.verifier)
    }
}

/// The handshake over a connected `socket`, `host` being the name (or address) the server's
/// certificate must carry.
pub async fn open<'a>(
    socket: TcpSocket<'a>,
    host: &str,
    read: &'a mut [u8],
    write: &'a mut [u8],
) -> Result<Connection<'a>, TlsError> {
    let config = TlsConfig::new().with_server_name(host);
    let provider = Provider { rng: Entropy(Rng::new()), verifier: CertVerifier::new(Certificate::X509(CA)) };
    let mut connection = TlsConnection::new(socket, read, write);
    connection.open(TlsContext::new(&config, provider)).await?;
    Ok(connection)
}
