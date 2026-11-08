//! HTTP-specific data structures.
//!
//! These mirror equivalent concepts from `prime_server`'s `http_protocol.hpp`.

use bitfield_struct::bitfield;
use zerocopy::{FromBytes, LE, U16, U32};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// HTTP request info (`prime_server`'s `http_request_info_t`).
///
/// `prime_server`'s HTTP server creates one of these for every request
/// and sends it as the first frame of every message about that request:
/// jobs passed between workers, results sent to the loopback,
/// and (as its first eight bytes) interrupts.
///
/// # Layout
///
/// The C++ struct is two `uint32_t` fields followed by a 16-bit bitfield,
/// which is 10 bytes of data,
/// but its alignment is that of `uint32_t`, so `sizeof` pads it to 12 bytes.
/// `prime_server` copies the struct's memory verbatim,
/// so the frame is 12 bytes in host byte order.
/// We read it as little-endian, which matches every platform Valhalla runs on.
/// The two trailing padding bytes carry no meaning;
/// `prime_server` does not promise to zero them, so we preserve whatever arrives.
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned, Debug, Copy, Clone)]
#[repr(C)]
pub struct HttpRequestInfo {
    /// The request ID, assigned serially by the server (wrapping at `u32::MAX`).
    id: U32<LE>,
    /// The time the server received the request, in integer seconds since the UNIX epoch.
    ///
    /// `prime_server` rounds `difftime(time(nullptr), 0)` to the nearest second.
    /// This will overflow in 2106.
    timestamp: U32<LE>,
    inner_bitfield: HttpRequestInfoInnerBitfield,
    /// Padding from the C++ struct's 4-byte alignment (see the type's documentation).
    _padding: U16<LE>,
}

#[bitfield(u16,
    repr = U16<LE>,
    from = bit_twiddling_helpers::conv_u16le::from_inner,
    into = bit_twiddling_helpers::conv_u16le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
struct HttpRequestInfoInnerBitfield {
    /// Protocol-specific space for versioning info.
    ///
    /// `prime_server` sets 0 for HTTP/1.0 and 1 for anything else,
    /// and reads any nonzero value as HTTP/1.1.
    #[bits(3)]
    version: u8,
    /// Whether the request had a `Connection: Keep-Alive` header.
    #[bits(1)]
    connection_keep_alive: bool,
    /// Whether the request had a `Connection: Close` header.
    #[bits(1)]
    connection_close: bool,
    /// The response code sent back to the client.
    ///
    /// Workers set this on the request info they send to the loopback,
    /// and the server uses it for logging.
    #[bits(10, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    response_code: U16<LE>,
    /// The bitfield's spare bit.
    #[bits(1)]
    _spare: u8,
}

/// The size of the request info frame.
pub(crate) const HTTP_REQUEST_INFO_SIZE: usize = size_of::<HttpRequestInfo>();
const _: () = assert!(HTTP_REQUEST_INFO_SIZE == 12);

impl HttpRequestInfo {
    /// Reads the request info from a message frame,
    /// or returns `None` if the frame is not exactly 12 bytes.
    #[inline]
    pub fn from_frame(frame: &[u8]) -> Option<Self> {
        Self::read_from_bytes(frame).ok()
    }

    /// The request ID (assigned serially by the server).
    #[inline]
    pub fn id(&self) -> u32 {
        self.id.get()
    }

    /// The time the server received the request, in seconds since the UNIX epoch.
    #[inline]
    pub fn timestamp(&self) -> u32 {
        self.timestamp.get()
    }

    /// The key `prime_server` uses to identify a request,
    /// both in its own bookkeeping and in interrupt messages:
    /// the ID in the low 32 bits and the timestamp in the high 32 bits.
    #[inline]
    pub fn key(&self) -> u64 {
        u64::from(self.id()) | (u64::from(self.timestamp()) << 32)
    }

    /// The HTTP version string for the response,
    /// as `prime_server`'s `http_response_t::from_info` chooses it.
    #[inline]
    pub fn http_version_string(&self) -> &'static str {
        if self.inner_bitfield.version() == 0 {
            "HTTP/1.0"
        } else {
            "HTTP/1.1"
        }
    }

    /// Whether the request had a `Connection: Keep-Alive` header.
    #[inline]
    pub fn connection_keep_alive(&self) -> bool {
        self.inner_bitfield.connection_keep_alive()
    }

    /// Whether the request had a `Connection: Close` header.
    #[inline]
    pub fn connection_close(&self) -> bool {
        self.inner_bitfield.connection_close()
    }

    /// The response code recorded for the request (0 until a worker responds).
    #[inline]
    pub fn response_code(&self) -> u16 {
        self.inner_bitfield.response_code().get()
    }

    /// Records the response code, as `prime_server`'s `http_response_t::from_info` does.
    ///
    /// HTTP status codes are three digits, so they always fit the 10-bit field.
    #[inline]
    pub(crate) fn set_response_code(&mut self, code: http::StatusCode) {
        self.inner_bitfield.set_response_code(code.as_u16().into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use zerocopy::{IntoBytes, transmute};

    #[test]
    fn decode_http_info_message() {
        const MESSAGE: [u8; 12] = [
            0x00, 0x00, 0x00, 0x00, 0xf5, 0x76, 0xb1, 0x68, 0x01, 0x00, 0x00, 0x00,
        ];
        let parsed: HttpRequestInfo = transmute!(MESSAGE);

        if !cfg!(miri) {
            insta::assert_debug_snapshot!(parsed);
        }
    }

    #[test]
    fn decode_captured_request_info() {
        // Request 2 of the Andorra captures, as mainline 3.9.1's server sent it to Loki.
        let frame = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/captures/andorra/auto_town/json/loki-thor.info.bin"),
        )
        .expect("fixture");
        let info = HttpRequestInfo::from_frame(&frame).expect("12 bytes");
        assert_eq!(info.id(), 2);
        assert_eq!(info.timestamp(), 0x6aef_c766);
        assert_eq!(info.key(), 0x6aef_c766_0000_0002);
        assert_eq!(info.http_version_string(), "HTTP/1.1");
        assert!(!info.connection_keep_alive());
        assert!(!info.connection_close());
        assert_eq!(info.response_code(), 0);
        assert_eq!(info.as_bytes(), frame.as_slice());
    }

    /// Mainline passes the request info through Loki and Thor untouched,
    /// so both boundaries of a request carry the same bytes.
    #[test]
    fn captured_request_info_is_unchanged_across_stages() {
        let captures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/captures");
        let mut checked = 0;
        for region in fs::read_dir(&captures).expect("captures") {
            for case in fs::read_dir(region.expect("region").path()).expect("cases") {
                for format in fs::read_dir(case.expect("case").path()).expect("formats") {
                    let dir = format.expect("format").path();
                    let loki_thor = fs::read(dir.join("loki-thor.info.bin")).expect("loki-thor");
                    let thor_odin = fs::read(dir.join("thor-odin.info.bin")).expect("thor-odin");
                    assert_eq!(loki_thor, thor_odin, "{}", dir.display());

                    let info = HttpRequestInfo::from_frame(&loki_thor).expect("12 bytes");
                    assert_eq!(info.http_version_string(), "HTTP/1.1");
                    assert_eq!(info.response_code(), 0, "{}", dir.display());
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 54);
    }

    #[test]
    fn rejects_wrong_sizes() {
        assert!(HttpRequestInfo::from_frame(&[0; 11]).is_none());
        assert!(HttpRequestInfo::from_frame(&[0; 13]).is_none());
        assert!(HttpRequestInfo::from_frame(&[]).is_none());
    }

    #[test]
    fn bitfield_layout_matches_prime_server() {
        // version = 0 (HTTP/1.0), keep-alive set, close clear: bits 0-2 = 0, bit 3 = 1.
        let info = HttpRequestInfo::from_frame(&[0, 0, 0, 0, 0, 0, 0, 0, 0b1000, 0, 0xab, 0xcd])
            .expect("12 bytes");
        assert_eq!(info.http_version_string(), "HTTP/1.0");
        assert!(info.connection_keep_alive());
        assert!(!info.connection_close());

        // close is bit 4.
        let info = HttpRequestInfo::from_frame(&[0, 0, 0, 0, 0, 0, 0, 0, 0b1_0001, 0, 0, 0])
            .expect("12 bytes");
        assert_eq!(info.http_version_string(), "HTTP/1.1");
        assert!(!info.connection_keep_alive());
        assert!(info.connection_close());

        // The response code occupies bits 5-14; setting it leaves the other bits
        // and the padding untouched.
        let mut info = HttpRequestInfo::from_frame(&[
            1,
            0,
            0,
            0,
            2,
            0,
            0,
            0,
            0b1_1001,
            0b1000_0000,
            0xab,
            0xcd,
        ])
        .expect("12 bytes");
        info.set_response_code(http::StatusCode::NOT_FOUND);
        assert_eq!(info.response_code(), 404);
        let bytes = info.as_bytes();
        // 404 << 5 = 0x3280; plus the low five bits 0b11001 and the spare bit 15.
        assert_eq!(
            u16::from_le_bytes([bytes[8], bytes[9]]),
            0x3280 | 0b1_1001 | 0x8000
        );
        assert_eq!(&bytes[10..], &[0xab, 0xcd]);
        assert_eq!(info.id(), 1);
        assert_eq!(info.timestamp(), 2);
    }
}
