use std::io::{Read, Seek, SeekFrom};

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const ZIP64_LOCATOR_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const SCAN_BUFFER_BYTES: usize = 128 * 1024;
const MAX_EOCD_SIGNATURES: usize = 1_024;
const MAX_EOCD_CANDIDATES: usize = 64;

#[derive(Clone, Copy, Debug)]
struct EndRecordCandidate {
    position: u64,
    end: u64,
    disk: u16,
    central_disk: u16,
    disk_entries: u16,
    total_entries: u16,
    central_size: u32,
    central_offset: u32,
    uses_zip64: bool,
}

#[derive(Clone, Copy, Debug)]
struct Zip64EndRecord {
    position: u64,
    disk: u32,
    central_disk: u32,
    disk_entries: u64,
    total_entries: u64,
    central_size: u64,
    central_offset: u64,
}

/// Returns the unambiguous, self-consistent central-directory entry count
/// before `zip::ZipArchive` can allocate an entry table.
///
/// The complete bounded archive is scanned instead of trusting the last EOCD
/// signature. This deliberately rejects ZIP polyglots and appended low-count
/// EOCD records that could make preflight and the ZIP parser select different
/// directory metadata.
pub(crate) fn advertised_zip_entry_count<R: Read + Seek>(reader: &mut R) -> Result<u64, String> {
    let result = advertised_zip_entry_count_inner(reader);
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("failed to rewind ZIP: {error}"))?;
    result
}

fn advertised_zip_entry_count_inner<R: Read + Seek>(reader: &mut R) -> Result<u64, String> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| format!("failed to determine ZIP length: {error}"))?;
    let signature_positions = scan_eocd_signatures(reader, file_len)?;
    let mut candidates = Vec::new();
    for position in signature_positions {
        if let Some(candidate) = read_eocd_candidate(reader, position, file_len)? {
            candidates.push(candidate);
            if candidates.len() > MAX_EOCD_CANDIDATES {
                return Err(format!(
                    "ZIP contains more than {MAX_EOCD_CANDIDATES} plausible end records"
                ));
            }
        }
    }

    let candidate = match candidates.as_slice() {
        [] => {
            return Err(
                "ZIP end-of-central-directory record is missing, truncated, or inconsistent"
                    .to_owned(),
            );
        }
        [candidate] => *candidate,
        _ => {
            return Err(
                "ZIP end-of-central-directory record is ambiguous; appended ZIP metadata is not supported"
                    .to_owned(),
            );
        }
    };
    if candidate.end != file_len {
        return Err("ZIP contains data after its end-of-central-directory record".to_owned());
    }
    if candidate.disk != 0 || candidate.central_disk != 0 {
        return Err("multi-disk ZIP archives are not supported".to_owned());
    }

    if !candidate.uses_zip64 {
        if candidate.disk_entries != candidate.total_entries {
            return Err("ZIP entry counts disagree across end records".to_owned());
        }
        let central_end = u64::from(candidate.central_offset)
            .checked_add(u64::from(candidate.central_size))
            .ok_or_else(|| "ZIP central-directory span overflows".to_owned())?;
        if central_end != candidate.position {
            return Err(
                "ZIP central directory does not end at its end-of-directory record".to_owned(),
            );
        }
        return Ok(u64::from(candidate.total_entries));
    }

    let zip64 = read_zip64_end_record(reader, candidate.position)?;
    if zip64.disk != 0 || zip64.central_disk != 0 || zip64.disk_entries != zip64.total_entries {
        return Err("ZIP64 entry counts or disk numbers are inconsistent".to_owned());
    }
    if !standard_u16_matches(candidate.disk_entries, zip64.disk_entries)
        || !standard_u16_matches(candidate.total_entries, zip64.total_entries)
        || !standard_u32_matches(candidate.central_size, zip64.central_size)
        || !standard_u32_matches(candidate.central_offset, zip64.central_offset)
    {
        return Err("ZIP and ZIP64 end records disagree".to_owned());
    }
    let central_end = zip64
        .central_offset
        .checked_add(zip64.central_size)
        .ok_or_else(|| "ZIP64 central-directory span overflows".to_owned())?;
    if central_end != zip64.position {
        return Err(
            "ZIP64 central directory does not end at its end-of-directory record".to_owned(),
        );
    }
    Ok(zip64.total_entries)
}

fn scan_eocd_signatures<R: Read + Seek>(reader: &mut R, file_len: u64) -> Result<Vec<u64>, String> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("failed to seek to ZIP start: {error}"))?;
    let mut positions = Vec::new();
    let mut buffer = vec![0_u8; SCAN_BUFFER_BYTES];
    let mut carry = Vec::with_capacity(3);
    let mut consumed = 0_u64;
    while consumed < file_len {
        let requested = usize::try_from((file_len - consumed).min(buffer.len() as u64))
            .expect("bounded scan buffer length fits usize");
        let count = reader
            .read(&mut buffer[..requested])
            .map_err(|error| format!("failed to scan ZIP end records: {error}"))?;
        if count == 0 {
            return Err("ZIP ended while scanning its end records".to_owned());
        }
        let carry_len = carry.len();
        let mut window = carry;
        window.extend_from_slice(&buffer[..count]);
        let window_start = consumed.saturating_sub(carry_len as u64);
        for index in 0..=window.len().saturating_sub(EOCD_SIGNATURE.len()) {
            if window[index..].starts_with(&EOCD_SIGNATURE) {
                positions.push(window_start + index as u64);
                if positions.len() > MAX_EOCD_SIGNATURES {
                    return Err(format!(
                        "ZIP contains more than {MAX_EOCD_SIGNATURES} end-record signatures"
                    ));
                }
            }
        }
        let carry_start = window.len().saturating_sub(3);
        carry = window[carry_start..].to_vec();
        consumed += count as u64;
    }
    Ok(positions)
}

fn read_eocd_candidate<R: Read + Seek>(
    reader: &mut R,
    position: u64,
    file_len: u64,
) -> Result<Option<EndRecordCandidate>, String> {
    if file_len.saturating_sub(position) < 22 {
        return Ok(None);
    }
    reader
        .seek(SeekFrom::Start(position))
        .map_err(|error| format!("failed to seek to ZIP end record: {error}"))?;
    let mut record = [0_u8; 22];
    reader
        .read_exact(&mut record)
        .map_err(|error| format!("failed to read ZIP end record: {error}"))?;
    if record[..4] != EOCD_SIGNATURE {
        return Ok(None);
    }
    let comment_len = u64::from(u16::from_le_bytes([record[20], record[21]]));
    let Some(end) = position
        .checked_add(record.len() as u64)
        .and_then(|value| value.checked_add(comment_len))
    else {
        return Ok(None);
    };
    if end > file_len {
        return Ok(None);
    }

    let disk_entries = u16::from_le_bytes([record[8], record[9]]);
    let total_entries = u16::from_le_bytes([record[10], record[11]]);
    let central_size = u32::from_le_bytes(record[12..16].try_into().expect("four bytes"));
    let central_offset = u32::from_le_bytes(record[16..20].try_into().expect("four bytes"));
    let uses_zip64 = disk_entries == u16::MAX
        || total_entries == u16::MAX
        || central_size == u32::MAX
        || central_offset == u32::MAX;
    let has_zip64_locator = uses_zip64
        && position >= 20
        && read_signature(reader, position - 20)? == ZIP64_LOCATOR_SIGNATURE;
    let standard_span_is_canonical = !uses_zip64
        && u64::from(central_offset)
            .checked_add(u64::from(central_size))
            .is_some_and(|central_end| central_end == position);

    // A record ending at EOF must be validated even when malformed. Earlier
    // signatures only matter when they describe a canonical directory span or
    // have an immediately preceding ZIP64 locator.
    if end != file_len && !standard_span_is_canonical && !has_zip64_locator {
        return Ok(None);
    }
    Ok(Some(EndRecordCandidate {
        position,
        end,
        disk: u16::from_le_bytes([record[4], record[5]]),
        central_disk: u16::from_le_bytes([record[6], record[7]]),
        disk_entries,
        total_entries,
        central_size,
        central_offset,
        uses_zip64,
    }))
}

fn read_zip64_end_record<R: Read + Seek>(
    reader: &mut R,
    standard_position: u64,
) -> Result<Zip64EndRecord, String> {
    let locator_position = standard_position
        .checked_sub(20)
        .ok_or_else(|| "ZIP64 locator is missing".to_owned())?;
    reader
        .seek(SeekFrom::Start(locator_position))
        .map_err(|error| format!("failed to seek to ZIP64 locator: {error}"))?;
    let mut locator = [0_u8; 20];
    reader
        .read_exact(&mut locator)
        .map_err(|error| format!("failed to read ZIP64 locator: {error}"))?;
    if locator[..4] != ZIP64_LOCATOR_SIGNATURE {
        return Err("ZIP64 locator is missing".to_owned());
    }
    let locator_disk = u32::from_le_bytes(locator[4..8].try_into().expect("four bytes"));
    let position = u64::from_le_bytes(locator[8..16].try_into().expect("eight bytes"));
    let disk_count = u32::from_le_bytes(locator[16..20].try_into().expect("four bytes"));
    if locator_disk != 0 || disk_count != 1 {
        return Err("multi-disk ZIP64 archives are not supported".to_owned());
    }
    reader
        .seek(SeekFrom::Start(position))
        .map_err(|error| format!("failed to seek to ZIP64 end record: {error}"))?;
    let mut record = [0_u8; 56];
    reader
        .read_exact(&mut record)
        .map_err(|error| format!("failed to read ZIP64 end record: {error}"))?;
    if record[..4] != ZIP64_EOCD_SIGNATURE {
        return Err("ZIP64 end-of-central-directory record is missing".to_owned());
    }
    let record_size = u64::from_le_bytes(record[4..12].try_into().expect("eight bytes"));
    if record_size < 44 {
        return Err("ZIP64 end-of-central-directory record is truncated".to_owned());
    }
    let record_end = position
        .checked_add(12)
        .and_then(|value| value.checked_add(record_size))
        .ok_or_else(|| "ZIP64 end-record length overflows".to_owned())?;
    if record_end != locator_position {
        return Err("ZIP64 end record and locator are not adjacent".to_owned());
    }
    Ok(Zip64EndRecord {
        position,
        disk: u32::from_le_bytes(record[16..20].try_into().expect("four bytes")),
        central_disk: u32::from_le_bytes(record[20..24].try_into().expect("four bytes")),
        disk_entries: u64::from_le_bytes(record[24..32].try_into().expect("eight bytes")),
        total_entries: u64::from_le_bytes(record[32..40].try_into().expect("eight bytes")),
        central_size: u64::from_le_bytes(record[40..48].try_into().expect("eight bytes")),
        central_offset: u64::from_le_bytes(record[48..56].try_into().expect("eight bytes")),
    })
}

fn read_signature<R: Read + Seek>(reader: &mut R, position: u64) -> Result<[u8; 4], String> {
    reader
        .seek(SeekFrom::Start(position))
        .map_err(|error| format!("failed to seek while checking ZIP end records: {error}"))?;
    let mut signature = [0_u8; 4];
    reader
        .read_exact(&mut signature)
        .map_err(|error| format!("failed to read ZIP end-record signature: {error}"))?;
    Ok(signature)
}

fn standard_u16_matches(standard: u16, actual: u64) -> bool {
    standard == u16::MAX || u64::from(standard) == actual
}

fn standard_u32_matches(standard: u32, actual: u64) -> bool {
    standard == u32::MAX || u64::from(standard) == actual
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek, Write};

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn archive(entry_count: usize) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..entry_count {
            writer
                .start_file(format!("entry-{index}"), SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"data").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn eocd_position(bytes: &[u8]) -> usize {
        bytes
            .windows(EOCD_SIGNATURE.len())
            .rposition(|window| window == EOCD_SIGNATURE)
            .unwrap()
    }

    fn with_zip64_offset_sentinel(mut bytes: Vec<u8>) -> Vec<u8> {
        let eocd = eocd_position(&bytes);
        let standard = bytes[eocd..eocd + 22].to_vec();
        let disk_entries = u16::from_le_bytes(standard[8..10].try_into().unwrap());
        let total_entries = u16::from_le_bytes(standard[10..12].try_into().unwrap());
        let central_size = u32::from_le_bytes(standard[12..16].try_into().unwrap());
        let central_offset = u32::from_le_bytes(standard[16..20].try_into().unwrap());
        bytes.truncate(eocd);

        let zip64_position = bytes.len() as u64;
        bytes.extend_from_slice(&ZIP64_EOCD_SIGNATURE);
        bytes.extend_from_slice(&44_u64.to_le_bytes());
        bytes.extend_from_slice(&45_u16.to_le_bytes());
        bytes.extend_from_slice(&45_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&u64::from(disk_entries).to_le_bytes());
        bytes.extend_from_slice(&u64::from(total_entries).to_le_bytes());
        bytes.extend_from_slice(&u64::from(central_size).to_le_bytes());
        bytes.extend_from_slice(&u64::from(central_offset).to_le_bytes());
        bytes.extend_from_slice(&ZIP64_LOCATOR_SIGNATURE);
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&zip64_position.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&EOCD_SIGNATURE);
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&disk_entries.to_le_bytes());
        bytes.extend_from_slice(&total_entries.to_le_bytes());
        bytes.extend_from_slice(&central_size.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn reads_standard_entry_count_and_rewinds() {
        let mut archive = Cursor::new(archive(3));
        assert_eq!(advertised_zip_entry_count(&mut archive).unwrap(), 3);
        assert_eq!(archive.stream_position().unwrap(), 0);
    }

    #[test]
    fn recognizes_zip64_when_only_offset_uses_a_sentinel() {
        let mut archive = Cursor::new(with_zip64_offset_sentinel(archive(3)));
        assert_eq!(advertised_zip_entry_count(&mut archive).unwrap(), 3);
        assert_eq!(archive.stream_position().unwrap(), 0);
    }

    #[test]
    fn rejects_missing_and_inconsistent_end_records() {
        assert!(advertised_zip_entry_count(&mut Cursor::new(b"not zip".to_vec())).is_err());

        let mut bytes = archive(1);
        let eocd = eocd_position(&bytes);
        bytes[eocd + 8..eocd + 10].copy_from_slice(&2_u16.to_le_bytes());
        assert!(advertised_zip_entry_count(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn rejects_an_appended_low_count_end_record_as_ambiguous() {
        let mut bytes = archive(3);
        let appended_position = u32::try_from(bytes.len()).unwrap();
        bytes.extend_from_slice(&EOCD_SIGNATURE);
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&appended_position.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());

        let error = advertised_zip_entry_count(&mut Cursor::new(bytes)).unwrap_err();
        assert!(error.contains("ambiguous"), "unexpected error: {error}");
    }
}
