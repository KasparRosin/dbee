//! `cargo xtask icns <out.icns> <in.png>…`: pack PNG images into a macOS `.icns`.
//!
//! macOS 10.7 and later read PNG payloads, so each PNG is stored as is under every icon type of
//! its size. This builds the `.icns` on any OS; `iconutil` exists only on macOS.

use std::path::Path;

use crate::ico::png_size;

/// Icon types and their pixel size, written in this order for each size present.
pub const TYPES: &[(&[u8; 4], u32)] = &[
    (b"ic10", 1024), // 512@2x
    (b"ic09", 512),
    (b"ic14", 512), // 256@2x
    (b"ic08", 256),
    (b"ic13", 256), // 128@2x
    (b"ic07", 128),
    (b"ic12", 64), // 32@2x
    (b"icp5", 32),
    (b"ic11", 32), // 16@2x
    (b"icp4", 16),
];

/// An `.icns` holding `pngs` (square; sizes without an icon type are skipped).
pub fn pack(pngs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let mut sized = Vec::with_capacity(pngs.len());
    for png in pngs {
        let (w, h) = png_size(png)?;
        if w != h {
            return Err(format!("icon images must be square, got {w}x{h}"));
        }
        sized.push((w, png));
    }
    let mut body = Vec::new();
    for (ty, size) in TYPES {
        let Some((_, png)) = sized.iter().find(|(w, _)| w == size) else { continue };
        let len = u32::try_from(png.len() + 8).map_err(|_| "a PNG is too large for .icns".to_string())?;
        body.extend_from_slice(*ty);
        body.extend_from_slice(&len.to_be_bytes());
        body.extend_from_slice(png);
    }
    if body.is_empty() {
        return Err("no PNG in a size .icns holds (16, 32, 64, 128, 256, 512, 1024)".into());
    }
    let total = u32::try_from(body.len() + 8).map_err(|_| ".icns too large".to_string())?;
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&total.to_be_bytes());
    out.extend(body);
    Ok(out)
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let [out, inputs @ ..] = args else {
        return Err("usage: cargo xtask icns <out.icns> <in.png>…".into());
    };
    let pngs = inputs.iter().map(|p| std::fs::read(p).map_err(|e| format!("read {p}: {e}"))).collect::<Result<Vec<_>, _>>()?;
    let icns = pack(&pngs)?;
    std::fs::write(Path::new(out), &icns).map_err(|e| format!("write {out}: {e}"))?;
    println!("{out}: {} bytes", icns.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_png(size: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4]);
        v
    }

    /// (type, payload length) of every entry.
    fn entries(icns: &[u8]) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut at = 8;
        while at + 8 <= icns.len() {
            let ty = String::from_utf8_lossy(&icns[at..at + 4]).into_owned();
            let len = u32::from_be_bytes(icns[at + 4..at + 8].try_into().unwrap()) as usize;
            out.push((ty, len - 8));
            at += len;
        }
        out
    }

    #[test]
    fn header_and_entries_by_size() {
        let p16 = fake_png(16);
        let p32 = fake_png(32);
        let icns = pack(&[p16.clone(), p32.clone()]).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        assert_eq!(entries(&icns), vec![("icp5".into(), p32.len()), ("ic11".into(), p32.len()), ("icp4".into(), p16.len())]);
    }

    #[test]
    fn payload_is_the_png_unchanged() {
        let p = fake_png(1024);
        let icns = pack(std::slice::from_ref(&p)).unwrap();
        assert_eq!(entries(&icns), vec![("ic10".into(), p.len())]);
        assert_eq!(&icns[16..], &p[..]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(pack(&[]).is_err());
        assert!(pack(&[fake_png(48)]).is_err(), "no icon type is 48 px");
        assert!(pack(&[b"GIF89a".to_vec()]).is_err());
        let mut wide = fake_png(32);
        wide[16..20].copy_from_slice(&64u32.to_be_bytes());
        assert!(pack(&[wide]).is_err(), "not square");
    }

    #[test]
    fn committed_icon_is_valid() {
        let path = crate::root().join("assets/app-icon/dbee.icns");
        let Ok(icns) = std::fs::read(&path) else { return };
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        assert!(entries(&icns).len() >= 8, "expected most sizes in {}", path.display());
    }
}
