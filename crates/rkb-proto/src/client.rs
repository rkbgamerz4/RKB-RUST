//! A blocking server-list ping.
//!
//! This is deliberately small: it proves framing and the status exchange
//! without claiming a login or gameplay. A later network layer wraps the same
//! codecs in a session thread.

use std::fmt;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::codec::Reader;
use crate::error::{Error, Result};
use crate::frame::FrameCodec;
use crate::ids;
use crate::packets::{
    Handshake, NextState, PingRequest, PongResponse, StatusRequest, StatusResponse,
};
use crate::status::ServerStatus;

/// The default Minecraft Java Edition port.
pub const DEFAULT_PORT: u16 = 25_565;

/// A parsed `host[:port]` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    pub host: String,
    pub port: u16,
}

impl ServerAddress {
    /// Parses `host`, `host:port`, `[ipv6]`, or `[ipv6]:port`.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Error::protocol("empty server address"));
        }

        if let Some(rest) = input.strip_prefix('[') {
            let (host, rest) = rest
                .split_once(']')
                .ok_or_else(|| Error::protocol("unterminated [IPv6] address"))?;
            if host.is_empty() {
                return Err(Error::protocol("empty host"));
            }
            let port = match rest {
                "" => DEFAULT_PORT,
                rest => {
                    let port = rest
                        .strip_prefix(':')
                        .ok_or_else(|| Error::protocol("junk after ] in address"))?;
                    parse_port(port)?
                }
            };
            return Ok(Self {
                host: host.to_owned(),
                port,
            });
        }

        let colons = input.bytes().filter(|byte| *byte == b':').count();
        if colons == 1 {
            let (host, port) = input
                .rsplit_once(':')
                .ok_or_else(|| Error::protocol("invalid server address"))?;
            if host.is_empty() {
                return Err(Error::protocol("empty host"));
            }
            return Ok(Self {
                host: host.to_owned(),
                port: parse_port(port)?,
            });
        }

        // No colon, or an unbracketed IPv6 literal.
        Ok(Self {
            host: input.to_owned(),
            port: DEFAULT_PORT,
        })
    }

    /// Resolves and connects within `timeout`.
    fn connect(&self, timeout: Duration) -> Result<TcpStream> {
        let timeout = timeout.max(Duration::from_millis(1));
        let addresses = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| Error::io(format!("{}: {error}", self.host)))?;
        let mut last_error = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    return Ok(stream);
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(Error::io(match last_error {
            Some(error) => format!("{}: {error}", self.host),
            None => format!("{} did not resolve to any address", self.host),
        }))
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse::<u16>()
        .map_err(|_| Error::protocol(format!("invalid port {port:?}")))
}

/// The result of a server-list ping.
#[derive(Clone, Debug, PartialEq)]
pub struct PingResult {
    pub address: ServerAddress,
    pub status: ServerStatus,
    pub raw_json: String,
    pub latency: Duration,
}

/// Pings `address` and returns its status document and round-trip time.
///
/// This performs the status exchange only. It does not log in, authenticate,
/// or join a world. DNS SRV records are not resolved yet; a host that needs one
/// must be given its final `host:port`.
pub fn ping(address: &str, timeout: Duration) -> Result<PingResult> {
    let address = ServerAddress::parse(address)?;
    let mut stream = address.connect(timeout)?;
    let codec = FrameCodec::uncompressed();

    let handshake = Handshake {
        protocol: crate::PROTOCOL_VERSION,
        host: address.host.clone(),
        port: address.port,
        next_state: NextState::Status,
    };
    codec.write_frame(&mut stream, &handshake.encode()?)?;
    codec.write_frame(&mut stream, &StatusRequest.encode())?;

    let response = codec.read_frame(&mut stream)?;
    let mut reader = Reader::new(&response);
    let id = reader.read_varint()?;
    if id != ids::status::CLIENTBOUND_RESPONSE {
        return Err(Error::UnknownPacket {
            state: "status",
            id,
        });
    }
    let response = StatusResponse::decode(reader.read_rest())?;
    let status = ServerStatus::parse(&response.json)?;

    let payload = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
        .min(i64::MAX as u128) as i64;
    let started = Instant::now();
    codec.write_frame(&mut stream, &PingRequest { payload }.encode())?;
    let echo = codec.read_frame(&mut stream)?;
    let latency = started.elapsed();
    let mut reader = Reader::new(&echo);
    let id = reader.read_varint()?;
    if id != ids::status::CLIENTBOUND_PONG {
        return Err(Error::UnknownPacket {
            state: "status",
            id,
        });
    }
    let echo = PongResponse::decode(reader.read_rest())?;
    if echo.payload != payload {
        return Err(Error::protocol(format!(
            "pong payload {} does not match ping payload {payload}",
            echo.payload
        )));
    }

    Ok(PingResult {
        address,
        status,
        raw_json: response.json,
        latency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_port_forms() {
        assert_eq!(
            ServerAddress::parse("example.com").unwrap(),
            ServerAddress {
                host: "example.com".to_owned(),
                port: 25_565
            }
        );
        assert_eq!(
            ServerAddress::parse("example.com:25566").unwrap(),
            ServerAddress {
                host: "example.com".to_owned(),
                port: 25_566
            }
        );
        assert_eq!(
            ServerAddress::parse("[::1]").unwrap(),
            ServerAddress {
                host: "::1".to_owned(),
                port: 25_565
            }
        );
        assert_eq!(
            ServerAddress::parse("[::1]:25567").unwrap(),
            ServerAddress {
                host: "::1".to_owned(),
                port: 25_567
            }
        );
        assert_eq!(
            ServerAddress::parse("::1").unwrap(),
            ServerAddress {
                host: "::1".to_owned(),
                port: 25_565
            }
        );
        assert_eq!(
            ServerAddress::parse("  example.com  ").unwrap().host,
            "example.com"
        );
    }

    #[test]
    fn rejects_bad_addresses() {
        assert!(ServerAddress::parse("").is_err());
        assert!(ServerAddress::parse(":25565").is_err());
        assert!(ServerAddress::parse("example.com:").is_err());
        assert!(ServerAddress::parse("example.com:70000").is_err());
        assert!(ServerAddress::parse("[::1").is_err());
        assert!(ServerAddress::parse("[::1]junk").is_err());
    }

    #[test]
    fn displays_ipv6_with_brackets() {
        let address = ServerAddress::parse("[::1]:25565").unwrap();
        assert_eq!(address.to_string(), "[::1]:25565");
    }

    #[test]
    fn ping_rejects_an_invalid_address_without_connecting() {
        assert!(ping("bad address:99999", Duration::from_secs(1)).is_err());
    }
}
