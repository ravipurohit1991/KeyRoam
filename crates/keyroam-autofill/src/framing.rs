//! Message framing shared by native messaging and the local bridge: a 32-bit
//! length in native byte order, followed by that many bytes of UTF-8 JSON.

use std::io::{self, Read, Write};

/// Largest message accepted in either direction. Chrome limits messages from
/// a host to 1 MiB, and no KeyRoam message comes close.
pub const MAX_MESSAGE: usize = 1024 * 1024;

/// Read one message. Returns `None` on a clean end of stream.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_frame(w: &mut impl Write, msg: &[u8]) -> io::Result<()> {
    if msg.len() > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = Vec::with_capacity(4 + msg.len());
    buf.extend_from_slice(&(msg.len() as u32).to_ne_bytes());
    buf.extend_from_slice(msg);
    let result = w.write_all(&buf).and_then(|_| w.flush());
    zeroize::Zeroize::zeroize(&mut buf);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_limits() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"a":1}"#).unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read_frame(&mut r).unwrap().unwrap(), br#"{"a":1}"#);
        assert_eq!(read_frame(&mut r).unwrap().unwrap(), b"");
        assert!(read_frame(&mut r).unwrap().is_none());

        // Truncated length or body.
        assert!(read_frame(&mut [1u8, 0].as_slice()).is_err());
        let mut short = Vec::new();
        short.extend_from_slice(&10u32.to_ne_bytes());
        short.extend_from_slice(b"abc");
        assert!(read_frame(&mut short.as_slice()).is_err());

        // Oversized messages are refused without allocating them.
        let huge = ((MAX_MESSAGE + 1) as u32).to_ne_bytes();
        assert!(read_frame(&mut huge.as_slice()).is_err());
        assert!(write_frame(&mut Vec::new(), &vec![0u8; MAX_MESSAGE + 1]).is_err());
    }
}
