//! A client's file report for the server's file check (`sv_pure`): Counter-Strike game files the
//! client read, each with its hash. Sent on the control lane (reliable and in order), compressed,
//! within a control frame's 16 KiB.

use sv_pure::{DIGEST_BYTES, Digest, FileKey, Game};

use super::wire::{WireError, WireReader, WireWriter};

const MAGIC: &[u8; 8] = b"IW4LPUR1";
/// Files in one report, at most.
pub(crate) const MAX_REPORT_FILES: usize = 160;
/// A report on the wire, at most (a control frame holds 16 KiB with its own header).
const MAX_REPORT_BYTES: usize = 12 * 1024;
/// A report's body before compression, at most.
const MAX_BODY_BYTES: usize = 96 * 1024;

fn put_text(out: &mut WireWriter, text: &str) {
    let text = &text.as_bytes()[..text.len().min(u16::MAX as usize)];
    out.put_u16(text.len() as u16);
    out.put_bytes(text);
}

fn get_text(input: &mut WireReader<'_>) -> Result<String, WireError> {
    let mut bytes = vec![0; usize::from(input.get_u16()?)];
    input.get_bytes(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| WireError::Malformed("file report path is not UTF-8"))
}

fn encode(files: &[(FileKey, Digest)]) -> Option<Vec<u8>> {
    let mut body = WireWriter::new();
    body.put_u16(files.len() as u16);
    for (key, digest) in files {
        body.put_u8(key.game.tag());
        put_text(&mut body, &key.pack);
        put_text(&mut body, &key.path);
        body.put_bytes(digest);
    }
    let body = body.finish();
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let packed = zstd::bulk::compress(&body, 3).ok()?;
    let mut out = Vec::with_capacity(MAGIC.len() + 4 + packed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&packed);
    (out.len() <= MAX_REPORT_BYTES).then_some(out)
}

/// A report of as many of `files` from the first on as fit (at least one, unless `files` is
/// empty or a single file cannot fit): its bytes and how many files it holds.
pub(crate) fn encode_pure_report(files: &[(FileKey, Digest)]) -> Option<(Vec<u8>, usize)> {
    let mut count = files.len().min(MAX_REPORT_FILES);
    while count > 0 {
        if let Some(bytes) = encode(&files[..count]) {
            return Some((bytes, count));
        }
        count /= 2;
    }
    None
}

/// The files of a report; `None` for bytes that are not one.
pub(crate) fn decode_pure_report(bytes: &[u8]) -> Result<Option<Vec<(FileKey, Digest)>>, WireError> {
    let Some(rest) = bytes.strip_prefix(MAGIC.as_slice()) else {
        return Ok(None);
    };
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(WireError::Malformed("file report size"));
    }
    let (len, packed) = rest
        .split_first_chunk::<4>()
        .ok_or(WireError::Malformed("file report header"))?;
    let len = u32::from_le_bytes(*len) as usize;
    if len > MAX_BODY_BYTES {
        return Err(WireError::Malformed("file report body size"));
    }
    let body = zstd::bulk::decompress(packed, len)
        .map_err(|_| WireError::Malformed("file report compression"))?;
    let mut input = WireReader::new(&body);
    let count = usize::from(input.get_u16()?);
    if count > MAX_REPORT_FILES {
        return Err(WireError::Malformed("file report count"));
    }
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let game =
            Game::from_tag(input.get_u8()?).ok_or(WireError::Malformed("file report game"))?;
        let pack = get_text(&mut input)?;
        let path = get_text(&mut input)?;
        let mut digest = [0; DIGEST_BYTES];
        input.get_bytes(&mut digest)?;
        files.push((FileKey { game, pack, path }, digest));
    }
    if !input.is_empty() {
        return Err(WireError::Malformed("trailing file report bytes"));
    }
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(index: usize) -> (FileKey, Digest) {
        let path = format!("models/player/custom_player/legacy/ctm_sas_variant{index}.mdl");
        let digest = sv_pure::digest(path.as_bytes());
        (
            FileKey {
                game: Game::Csgo,
                pack: "csgo/pak01_dir.vpk".to_owned(),
                path,
            },
            digest,
        )
    }

    #[test]
    fn reports_round_trip_and_fit_a_control_frame() {
        let files: Vec<_> = (0..500).map(file).collect();
        let (bytes, count) = encode_pure_report(&files).unwrap();
        assert!(count > 0 && count <= MAX_REPORT_FILES);
        assert!(bytes.len() <= MAX_REPORT_BYTES);
        assert_eq!(decode_pure_report(&bytes).unwrap().unwrap(), files[..count]);
        assert_eq!(decode_pure_report(b"IW4LACP1 not ours").unwrap(), None);
        assert!(decode_pure_report(b"IW4LPUR1\x05").is_err());
    }
}
