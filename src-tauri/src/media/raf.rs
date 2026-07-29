use std::io::{Read, Seek, SeekFrom};

use crate::domain::{DomainError, ErrorCode};

const RAF_MAGIC: &[u8] = b"FUJIFILMCCD-RAW ";
const PREVIEW_OFFSET_FIELD: u64 = 84;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RafPreview {
    pub offset: u64,
    pub length: u64,
}

pub fn locate_embedded_jpeg<R: Read + Seek>(
    reader: &mut R,
) -> Result<Option<RafPreview>, DomainError> {
    reader.seek(SeekFrom::Start(0)).map_err(media_error)?;
    let mut magic = [0_u8; 16];
    if reader.read_exact(&mut magic).is_err() || magic != RAF_MAGIC {
        return Err(DomainError::new(
            ErrorCode::UnsupportedMedia,
            "文件扩展名为 RAF，但文件头不是 Fuji RAF。",
        ));
    }
    reader
        .seek(SeekFrom::Start(PREVIEW_OFFSET_FIELD))
        .map_err(media_error)?;
    let mut fields = [0_u8; 8];
    reader.read_exact(&mut fields).map_err(media_error)?;
    let offset = u64::from(u32::from_be_bytes(fields[0..4].try_into().unwrap()));
    let length = u64::from(u32::from_be_bytes(fields[4..8].try_into().unwrap()));
    if offset == 0 || length < 4 {
        return Ok(None);
    }
    let total = reader.seek(SeekFrom::End(0)).map_err(media_error)?;
    let Some(end) = offset.checked_add(length) else {
        return Ok(None);
    };
    if end > total {
        return Ok(None);
    }
    reader.seek(SeekFrom::Start(offset)).map_err(media_error)?;
    let mut boundaries = [0_u8; 2];
    reader.read_exact(&mut boundaries).map_err(media_error)?;
    if boundaries != [0xff, 0xd8] {
        return Ok(None);
    }
    reader.seek(SeekFrom::Start(end - 2)).map_err(media_error)?;
    reader.read_exact(&mut boundaries).map_err(media_error)?;
    if boundaries != [0xff, 0xd9] {
        return Ok(None);
    }
    Ok(Some(RafPreview { offset, length }))
}

fn media_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "无法读取 RAF 内嵌预览。")
        .with_diagnostic(error.to_string())
}
