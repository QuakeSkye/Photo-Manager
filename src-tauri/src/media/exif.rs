use std::io::{Read, Seek};

use crate::domain::{DomainError, ErrorCode};

const MAX_EXIF_SEGMENT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub numerator: i64,
    pub denominator: i64,
}

impl Rational {
    fn new(numerator: i64, denominator: i64) -> Option<Self> {
        if denominator == 0 {
            return None;
        }
        let divisor = gcd(numerator.unsigned_abs(), denominator.unsigned_abs()).max(1);
        let divisor = i64::try_from(divisor).ok()?;
        let mut numerator = numerator / divisor;
        let mut denominator = denominator / divisor;
        if denominator < 0 {
            numerator = numerator.checked_neg()?;
            denominator = denominator.checked_neg()?;
        }
        Some(Self {
            numerator,
            denominator,
        })
    }
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let next = left % right;
        left = right;
        right = next;
    }
    left
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmSimulation {
    pub key: String,
    pub code: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureMetadata {
    pub lens_model: Option<String>,
    pub f_number: Option<Rational>,
    pub exposure_time: Option<Rational>,
    pub iso: Option<u32>,
    pub focal_length: Option<Rational>,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub film_simulation: Option<FilmSimulation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExifMetadata {
    pub capture_local_key: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
    pub capture: CaptureMetadata,
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

    let mut metadata = ExifMetadata::default();
    let mut parsed_exif = false;
    loop {
        let marker = read_marker(reader)?;
        if marker == 0xda || marker == 0xd9 {
            return Ok(metadata);
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
        if !parsed_exif && marker == 0xe1 && segment.starts_with(b"Exif\0\0") {
            metadata = parse_tiff(&segment[6..])?;
            parsed_exif = true;
        } else if is_start_of_frame(marker) && segment.len() >= 5 {
            let height = u32::from(u16::from_be_bytes([segment[1], segment[2]]));
            let width = u32::from(u16::from_be_bytes([segment[3], segment[4]]));
            if metadata.capture.pixel_width.is_none() && width > 0 {
                metadata.capture.pixel_width = Some(width);
            }
            if metadata.capture.pixel_height.is_none() && height > 0 {
                metadata.capture.pixel_height = Some(height);
            }
        }
    }
}

fn is_start_of_frame(marker: u8) -> bool {
    matches!(
        marker,
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf
    )
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
    let lens_model = read_ascii_tag(bytes, &exif_entries, 0xa434, endian);
    let f_number =
        read_rational_tag(bytes, &exif_entries, 0x829d, endian).filter(|value| value.numerator > 0);
    let exposure_time =
        read_rational_tag(bytes, &exif_entries, 0x829a, endian).filter(|value| value.numerator > 0);
    let iso = read_unsigned_tag(bytes, &exif_entries, 0x8827, endian).filter(|value| *value > 0);
    let focal_length =
        read_rational_tag(bytes, &exif_entries, 0x920a, endian).filter(|value| value.numerator > 0);
    let pixel_width =
        read_unsigned_tag(bytes, &exif_entries, 0xa002, endian).filter(|value| *value > 0);
    let pixel_height =
        read_unsigned_tag(bytes, &exif_entries, 0xa003, endian).filter(|value| *value > 0);
    let film_simulation = exif_entries
        .iter()
        .find(|entry| entry.tag == 0x927c)
        .and_then(|entry| entry.raw_data(bytes, endian))
        .and_then(parse_fujifilm_maker_note);

    Ok(ExifMetadata {
        capture_local_key,
        camera_model: model,
        camera_serial: serial,
        capture: CaptureMetadata {
            lens_model,
            f_number,
            exposure_time,
            iso,
            focal_length,
            pixel_width,
            pixel_height,
            film_simulation,
        },
    })
}

fn parse_fujifilm_maker_note(bytes: &[u8]) -> Option<FilmSimulation> {
    if bytes.len() < 12 || !bytes.starts_with(b"FUJIFILM") {
        return None;
    }
    let offset = usize::try_from(u32::from_le_bytes(bytes[8..12].try_into().ok()?)).ok()?;
    let entries = read_ifd(bytes, offset, Endian::Little).ok()?;
    let film_mode = entries
        .iter()
        .find(|entry| entry.tag == 0x1401)
        .and_then(|entry| entry.scalar_u32(Endian::Little));
    let saturation = entries
        .iter()
        .find(|entry| entry.tag == 0x1003)
        .and_then(|entry| entry.scalar_u32(Endian::Little));

    if let Some(code) = film_mode
        && let Some(key) = known_color_film_mode(code)
    {
        return Some(film_simulation(key, code));
    }
    if let Some(code) = saturation
        && let Some(key) = known_monochrome_mode(code)
    {
        return Some(film_simulation(key, code));
    }
    film_mode.map(|code| film_simulation("unknown", code))
}

fn film_simulation(key: &str, code: u32) -> FilmSimulation {
    FilmSimulation {
        key: key.to_string(),
        code,
    }
}

fn known_color_film_mode(code: u32) -> Option<&'static str> {
    match code {
        0x000 => Some("provia"),
        0x120 => Some("astia"),
        0x200 | 0x400 => Some("velvia"),
        0x500 => Some("pro-neg-std"),
        0x501 => Some("pro-neg-hi"),
        0x600 => Some("classic-chrome"),
        0x700 => Some("eterna"),
        0x800 => Some("classic-negative"),
        0x900 => Some("eterna-bleach-bypass"),
        0xa00 => Some("nostalgic-neg"),
        0xb00 => Some("reala-ace"),
        _ => None,
    }
}

fn known_monochrome_mode(code: u32) -> Option<&'static str> {
    match code {
        0x300 => Some("monochrome"),
        0x301 => Some("monochrome-red"),
        0x302 => Some("monochrome-yellow"),
        0x303 => Some("monochrome-green"),
        0x310 => Some("sepia"),
        0x500 => Some("acros"),
        0x501 => Some("acros-red"),
        0x502 => Some("acros-yellow"),
        0x503 => Some("acros-green"),
        _ => None,
    }
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
        if self.count != 1 {
            return None;
        }
        match self.field_type {
            3 => Some(u32::from(endian.u16(&self.value[..2]).ok()?)),
            4 => endian.u32(&self.value).ok(),
            _ => None,
        }
    }

    fn raw_data(self, bytes: &[u8], endian: Endian) -> Option<&[u8]> {
        let unit_size = match self.field_type {
            1 | 2 | 7 => 1,
            3 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        };
        let count = usize::try_from(self.count).ok()?;
        let length = count.checked_mul(unit_size)?;
        if length == 0 || length > MAX_EXIF_SEGMENT || length <= 4 {
            return None;
        }
        let offset = usize::try_from(endian.u32(&self.value).ok()?).ok()?;
        bytes.get(offset..offset.checked_add(length)?)
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
    let entry = *entries
        .iter()
        .find(|entry| entry.tag == tag && entry.field_type == 2)?;
    let count = usize::try_from(entry.count).ok()?;
    if count == 0 || count > MAX_EXIF_SEGMENT {
        return None;
    }
    let inline;
    let raw = if count <= 4 {
        inline = entry.value;
        &inline[..count]
    } else {
        entry.raw_data(bytes, endian)?
    };
    let end = raw
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(raw.len());
    let value = String::from_utf8_lossy(&raw[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn read_unsigned_tag(_bytes: &[u8], entries: &[IfdEntry], tag: u16, endian: Endian) -> Option<u32> {
    entries
        .iter()
        .find(|entry| entry.tag == tag)
        .and_then(|entry| entry.scalar_u32(endian))
}

fn read_rational_tag(
    bytes: &[u8],
    entries: &[IfdEntry],
    tag: u16,
    endian: Endian,
) -> Option<Rational> {
    let entry = *entries
        .iter()
        .find(|entry| entry.tag == tag && entry.count == 1)?;
    let raw = entry.raw_data(bytes, endian)?;
    match entry.field_type {
        5 => Rational::new(
            i64::from(endian.u32(raw.get(..4)?).ok()?),
            i64::from(endian.u32(raw.get(4..8)?).ok()?),
        ),
        10 => Rational::new(
            i64::from(endian.i32(raw.get(..4)?).ok()?),
            i64::from(endian.i32(raw.get(4..8)?).ok()?),
        ),
        _ => None,
    }
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

    fn i32(self, bytes: &[u8]) -> Result<i32, DomainError> {
        let bytes: [u8; 4] = bytes.try_into().map_err(|_| invalid_exif())?;
        Ok(match self {
            Self::Little => i32::from_le_bytes(bytes),
            Self::Big => i32::from_be_bytes(bytes),
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
        jpeg_with_tiff(generated_date_tiff(date), None)
    }

    fn generated_date_tiff(date: &str) -> Vec<u8> {
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
        tiff
    }

    fn jpeg_with_tiff(tiff: Vec<u8>, dimensions: Option<(u16, u16)>) -> Vec<u8> {
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend(tiff);
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend(payload);
        if let Some((width, height)) = dimensions {
            jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x07, 0x08]);
            jpeg.extend_from_slice(&height.to_be_bytes());
            jpeg.extend_from_slice(&width.to_be_bytes());
        }
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    fn generated_capture_tiff() -> Vec<u8> {
        fn add_offset_entry(
            tiff: &mut Vec<u8>,
            data: &mut Vec<u8>,
            data_offset: usize,
            tag: u16,
            field_type: u16,
            count: u32,
            value: &[u8],
        ) {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&field_type.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(
                &u32::try_from(data_offset + data.len())
                    .unwrap()
                    .to_le_bytes(),
            );
            data.extend_from_slice(value);
        }

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

        let entry_count = 7_u16;
        tiff.extend_from_slice(&entry_count.to_le_bytes());
        let data_offset = 26 + 2 + usize::from(entry_count) * 12 + 4;
        let mut data = Vec::new();
        add_offset_entry(
            &mut tiff,
            &mut data,
            data_offset,
            0xa434,
            2,
            12,
            b"XF35mmF1.4\0",
        );
        add_offset_entry(
            &mut tiff,
            &mut data,
            data_offset,
            0x829d,
            5,
            1,
            &[14, 0, 0, 0, 5, 0, 0, 0],
        );
        add_offset_entry(
            &mut tiff,
            &mut data,
            data_offset,
            0x829a,
            5,
            1,
            &[1, 0, 0, 0, 244, 1, 0, 0],
        );
        tiff.extend_from_slice(&0x8827_u16.to_le_bytes());
        tiff.extend_from_slice(&3_u16.to_le_bytes());
        tiff.extend_from_slice(&1_u32.to_le_bytes());
        tiff.extend_from_slice(&400_u16.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        add_offset_entry(
            &mut tiff,
            &mut data,
            data_offset,
            0x920a,
            5,
            1,
            &[35, 0, 0, 0, 1, 0, 0, 0],
        );
        for (tag, value) in [(0xa002_u16, 6240_u32), (0xa003_u16, 4160_u32)] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&4_u16.to_le_bytes());
            tiff.extend_from_slice(&1_u32.to_le_bytes());
            tiff.extend_from_slice(&value.to_le_bytes());
        }
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend(data);
        tiff
    }

    fn generated_big_endian_capture_tiff() -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&42_u16.to_be_bytes());
        tiff.extend_from_slice(&8_u32.to_be_bytes());
        tiff.extend_from_slice(&1_u16.to_be_bytes());
        tiff.extend_from_slice(&0x8769_u16.to_be_bytes());
        tiff.extend_from_slice(&4_u16.to_be_bytes());
        tiff.extend_from_slice(&1_u32.to_be_bytes());
        tiff.extend_from_slice(&26_u32.to_be_bytes());
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        tiff.extend_from_slice(&2_u16.to_be_bytes());
        tiff.extend_from_slice(&0x8827_u16.to_be_bytes());
        tiff.extend_from_slice(&3_u16.to_be_bytes());
        tiff.extend_from_slice(&1_u32.to_be_bytes());
        tiff.extend_from_slice(&400_u16.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0x829d_u16.to_be_bytes());
        tiff.extend_from_slice(&5_u16.to_be_bytes());
        tiff.extend_from_slice(&1_u32.to_be_bytes());
        tiff.extend_from_slice(&56_u32.to_be_bytes());
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(&28_u32.to_be_bytes());
        tiff.extend_from_slice(&10_u32.to_be_bytes());
        tiff
    }

    fn generated_fuji_maker_note(film_mode: Option<u16>, saturation: Option<u16>) -> Vec<u8> {
        let count = usize::from(film_mode.is_some()) + usize::from(saturation.is_some());
        let mut note = b"FUJIFILM".to_vec();
        note.extend_from_slice(&12_u32.to_le_bytes());
        note.extend_from_slice(&u16::try_from(count).unwrap().to_le_bytes());
        for (tag, value) in [(0x1401, film_mode), (0x1003, saturation)] {
            if let Some(value) = value {
                note.extend_from_slice(&u16::try_from(tag).unwrap().to_le_bytes());
                note.extend_from_slice(&3_u16.to_le_bytes());
                note.extend_from_slice(&1_u32.to_le_bytes());
                note.extend_from_slice(&value.to_le_bytes());
                note.extend_from_slice(&[0, 0]);
            }
        }
        note.extend_from_slice(&0_u32.to_le_bytes());
        note
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

    #[test]
    fn reads_requested_capture_parameters_and_reduces_rationals() {
        let jpeg = jpeg_with_tiff(generated_capture_tiff(), None);
        let capture = read_jpeg_exif(&mut Cursor::new(jpeg)).unwrap().capture;
        assert_eq!(capture.lens_model.as_deref(), Some("XF35mmF1.4"));
        assert_eq!(capture.f_number, Rational::new(14, 5));
        assert_eq!(capture.exposure_time, Rational::new(1, 500));
        assert_eq!(capture.iso, Some(400));
        assert_eq!(capture.focal_length, Rational::new(35, 1));
        assert_eq!(
            (capture.pixel_width, capture.pixel_height),
            (Some(6240), Some(4160))
        );
    }

    #[test]
    fn reads_big_endian_capture_parameters() {
        let capture = parse_tiff(&generated_big_endian_capture_tiff())
            .unwrap()
            .capture;
        assert_eq!(capture.iso, Some(400));
        assert_eq!(capture.f_number, Rational::new(14, 5));
    }

    #[test]
    fn uses_sof_dimensions_when_exif_dimensions_are_missing() {
        let jpeg = jpeg_with_tiff(
            generated_date_tiff("2026:07:29 12:34:56"),
            Some((3000, 4500)),
        );
        let capture = read_jpeg_exif(&mut Cursor::new(jpeg)).unwrap().capture;
        assert_eq!(
            (capture.pixel_width, capture.pixel_height),
            (Some(3000), Some(4500))
        );
    }

    #[test]
    fn maps_fuji_color_and_monochrome_modes_without_guessing() {
        for (code, key) in [
            (0x000, "provia"),
            (0x200, "velvia"),
            (0x600, "classic-chrome"),
            (0x700, "eterna"),
            (0x800, "classic-negative"),
            (0x900, "eterna-bleach-bypass"),
            (0xa00, "nostalgic-neg"),
            (0xb00, "reala-ace"),
        ] {
            assert_eq!(
                parse_fujifilm_maker_note(&generated_fuji_maker_note(Some(code), None))
                    .unwrap()
                    .key,
                key
            );
        }
        assert_eq!(
            parse_fujifilm_maker_note(&generated_fuji_maker_note(None, Some(0x500)))
                .unwrap()
                .key,
            "acros"
        );
        assert_eq!(
            parse_fujifilm_maker_note(&generated_fuji_maker_note(Some(0xdead), None)).unwrap(),
            film_simulation("unknown", 0xdead)
        );
    }

    #[test]
    fn zero_denominator_and_out_of_bounds_values_are_field_local_failures() {
        assert_eq!(Rational::new(1, 0), None);
        let entry = IfdEntry {
            tag: 1,
            field_type: 5,
            count: 1,
            value: u32::MAX.to_le_bytes(),
        };
        assert!(entry.raw_data(&[0; 16], Endian::Little).is_none());
    }
}
