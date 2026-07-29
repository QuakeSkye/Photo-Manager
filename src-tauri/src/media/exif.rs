use std::io::{Read, Seek};

use crate::domain::{DomainError, ErrorCode};

const MAX_EXIF_SEGMENT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExifMetadata {
    pub capture_local_key: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
}

pub fn read_jpeg_exif<R: Read + Seek>(reader: &mut R) -> Result<ExifMetadata, DomainError> {
    let mut soi = [0_u8; 2];
    reader.read_exact(&mut soi).map_err(media_error)?;
    if soi != [0xff, 0xd8] {
        return Err(DomainError::new(
            ErrorCode::UnsupportedMedia,
            "文件不是 JPEG。",
        ));
    }
    loop {
        let marker = read_marker(reader)?;
        if marker == 0xda || marker == 0xd9 {
            return Ok(ExifMetadata::default());
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let mut length = [0_u8; 2];
        reader.read_exact(&mut length).map_err(media_error)?;
        let length = usize::from(u16::from_be_bytes(length));
        if length < 2 || length - 2 > MAX_EXIF_SEGMENT {
            return Err(DomainError::new(
                ErrorCode::MediaUnreadable,
                "JPEG 元数据段长度无效。",
            ));
        }
        let mut segment = vec![0; length - 2];
        reader.read_exact(&mut segment).map_err(media_error)?;
        if marker == 0xe1 && segment.starts_with(b"Exif\0\0") {
            return parse_tiff(&segment[6..]);
        }
    }
}

fn read_marker<R: Read>(reader: &mut R) -> Result<u8, DomainError> {
    let mut value = [0_u8; 1];
    loop {
        reader.read_exact(&mut value).map_err(media_error)?;
        if value[0] != 0xff {
            continue;
        }
        loop {
            reader.read_exact(&mut value).map_err(media_error)?;
            if value[0] != 0xff {
                break;
            }
        }
        if value[0] != 0x00 {
            return Ok(value[0]);
        }
    }
}

fn parse_tiff(bytes: &[u8]) -> Result<ExifMetadata, DomainError> {
    if bytes.len() < 8 {
        return Err(invalid_exif());
    }
    let endian = match &bytes[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return Err(invalid_exif()),
    };
    if endian.u16(&bytes[2..4])? != 42 {
        return Err(invalid_exif());
    }
    let ifd0 = usize::try_from(endian.u32(&bytes[4..8])?).map_err(|_| invalid_exif())?;
    let entries = read_ifd(bytes, ifd0, endian)?;
    let model = read_ascii_tag(bytes, &entries, 0x0110, endian);
    let exif_offset = entries
        .iter()
        .find(|entry| entry.tag == 0x8769)
        .and_then(|entry| entry.scalar_u32(endian))
        .and_then(|value| usize::try_from(value).ok());
    let exif_entries = exif_offset
        .map(|offset| read_ifd(bytes, offset, endian))
        .transpose()?
        .unwrap_or_default();
    let raw_date = read_ascii_tag(bytes, &exif_entries, 0x9003, endian);
    let fraction = read_ascii_tag(bytes, &exif_entries, 0x9291, endian);
    let capture_local_key = raw_date
        .as_deref()
        .and_then(normalize_exif_date)
        .map(|mut value| {
            if let Some(fraction) = fraction
                .as_deref()
                .filter(|value| value.chars().all(|c| c.is_ascii_digit()))
            {
                value.push('.');
                value.push_str(fraction);
            }
            value
        });
    let serial = read_ascii_tag(bytes, &exif_entries, 0xa431, endian);
    Ok(ExifMetadata {
        capture_local_key,
        camera_model: model,
        camera_serial: serial,
    })
}

fn normalize_exif_date(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b':'
        || bytes[7] != b':'
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !bytes[..19]
            .iter()
            .enumerate()
            .all(|(index, value)| matches!(index, 4 | 7 | 10 | 13 | 16) || value.is_ascii_digit())
    {
        return None;
    }
    let parse = |range: std::ops::Range<usize>| value[range].parse::<u32>().ok();
    let year = parse(0..4)?;
    let month = parse(5..7)?;
    let day = parse(8..10)?;
    let hour = parse(11..13)?;
    let minute = parse(14..16)?;
    let second = parse(17..19)?;
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
    ))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => 31,
    }
}

#[derive(Debug, Clone, Copy)]
struct IfdEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    value: [u8; 4],
}

impl IfdEntry {
    fn scalar_u32(self, endian: Endian) -> Option<u32> {
        (self.field_type == 4 && self.count == 1)
            .then(|| endian.u32(&self.value).ok())
            .flatten()
    }
}

fn read_ifd(bytes: &[u8], offset: usize, endian: Endian) -> Result<Vec<IfdEntry>, DomainError> {
    let count_bytes = bytes.get(offset..offset + 2).ok_or_else(invalid_exif)?;
    let count = usize::from(endian.u16(count_bytes)?);
    let end = offset
        .checked_add(2)
        .and_then(|value| value.checked_add(count.checked_mul(12)?))
        .ok_or_else(invalid_exif)?;
    if end > bytes.len() {
        return Err(invalid_exif());
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset + 2 + index * 12;
        entries.push(IfdEntry {
            tag: endian.u16(&bytes[start..start + 2])?,
            field_type: endian.u16(&bytes[start + 2..start + 4])?,
            count: endian.u32(&bytes[start + 4..start + 8])?,
            value: bytes[start + 8..start + 12].try_into().unwrap(),
        });
    }
    Ok(entries)
}

fn read_ascii_tag(bytes: &[u8], entries: &[IfdEntry], tag: u16, endian: Endian) -> Option<String> {
    let entry = entries
        .iter()
        .find(|entry| entry.tag == tag && entry.field_type == 2)?;
    let count = usize::try_from(entry.count).ok()?;
    if count == 0 || count > MAX_EXIF_SEGMENT {
        return None;
    }
    let raw = if count <= 4 {
        &entry.value[..count]
    } else {
        let offset = usize::try_from(endian.u32(&entry.value).ok()?).ok()?;
        bytes.get(offset..offset.checked_add(count)?)?
    };
    let end = raw
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(raw.len());
    let value = String::from_utf8_lossy(&raw[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn u16(self, bytes: &[u8]) -> Result<u16, DomainError> {
        let bytes: [u8; 2] = bytes.try_into().map_err(|_| invalid_exif())?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        })
    }

    fn u32(self, bytes: &[u8]) -> Result<u32, DomainError> {
        let bytes: [u8; 4] = bytes.try_into().map_err(|_| invalid_exif())?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        })
    }
}

fn invalid_exif() -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "EXIF 结构无效。")
}

fn media_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "无法读取 JPEG EXIF。")
        .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn generated_jpeg(date: &str) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42_u16.to_le_bytes());
        tiff.extend_from_slice(&8_u32.to_le_bytes());
        tiff.extend_from_slice(&1_u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769_u16.to_le_bytes());
        tiff.extend_from_slice(&4_u16.to_le_bytes());
        tiff.extend_from_slice(&1_u32.to_le_bytes());
        tiff.extend_from_slice(&26_u32.to_le_bytes());
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend_from_slice(&1_u16.to_le_bytes());
        tiff.extend_from_slice(&0x9003_u16.to_le_bytes());
        tiff.extend_from_slice(&2_u16.to_le_bytes());
        tiff.extend_from_slice(&20_u32.to_le_bytes());
        tiff.extend_from_slice(&44_u32.to_le_bytes());
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend_from_slice(date.as_bytes());
        tiff.push(0);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend(tiff);
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend(payload);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    #[test]
    fn reads_reliable_local_capture_time_without_timezone_conversion() {
        let jpeg = generated_jpeg("2026:07:29 12:34:56");
        let metadata = read_jpeg_exif(&mut Cursor::new(jpeg)).unwrap();
        assert_eq!(
            metadata.capture_local_key.as_deref(),
            Some("2026-07-29T12:34:56")
        );
    }

    #[test]
    fn invalid_calendar_date_is_unknown() {
        let jpeg = generated_jpeg("2026:02:31 12:34:56");
        let metadata = read_jpeg_exif(&mut Cursor::new(jpeg)).unwrap();
        assert!(metadata.capture_local_key.is_none());
    }
}
