mod exif;
mod raf;

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::{
    catalog::MediaKind,
    domain::{DomainError, ErrorCode},
};

pub use exif::{CaptureMetadata, ExifMetadata, FilmSimulation, Rational};
pub use raf::RafPreview;

pub const MEDIA_PARSER_VERSION: i64 = 2;
const MAX_PREVIEW_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewState {
    JpegSource,
    RafEmbeddedJpeg { offset: u64, length: u64 },
    Unavailable,
}

impl PreviewState {
    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::JpegSource => "JpegSource",
            Self::RafEmbeddedJpeg { .. } => "RafEmbeddedJpeg",
            Self::Unavailable => "Unavailable",
        }
    }

    pub const fn offset(self) -> Option<i64> {
        match self {
            Self::RafEmbeddedJpeg { offset, .. } => Some(offset as i64),
            _ => None,
        }
    }

    pub const fn length(self) -> Option<i64> {
        match self {
            Self::RafEmbeddedJpeg { length, .. } => Some(length as i64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInspection {
    pub exif: ExifMetadata,
    pub preview: PreviewState,
}

pub fn inspect(file: &mut File, kind: MediaKind) -> Result<MediaInspection, DomainError> {
    file.seek(SeekFrom::Start(0)).map_err(media_io_error)?;
    match kind {
        MediaKind::Jpg => Ok(MediaInspection {
            exif: exif::read_jpeg_exif(file)?,
            preview: PreviewState::JpegSource,
        }),
        MediaKind::Raf => {
            let preview = match raf::locate_embedded_jpeg(file)? {
                Some(preview) => preview,
                None => {
                    return Ok(MediaInspection {
                        exif: ExifMetadata::default(),
                        preview: PreviewState::Unavailable,
                    });
                }
            };
            file.seek(SeekFrom::Start(preview.offset))
                .map_err(media_io_error)?;
            let mut bounded = file.take(preview.length);
            let exif = exif::read_jpeg_exif(&mut bounded).unwrap_or_default();
            Ok(MediaInspection {
                exif,
                preview: PreviewState::RafEmbeddedJpeg {
                    offset: preview.offset,
                    length: preview.length,
                },
            })
        }
    }
}

pub fn read_preview_bytes(file: &mut File, preview: PreviewState) -> Result<Vec<u8>, DomainError> {
    let (offset, length) = match preview {
        PreviewState::JpegSource => {
            let length = file.metadata().map_err(media_io_error)?.len();
            (0, length)
        }
        PreviewState::RafEmbeddedJpeg { offset, length } => (offset, length),
        PreviewState::Unavailable => {
            return Err(DomainError::new(
                ErrorCode::MediaUnreadable,
                "RAF 没有可用的内嵌 JPEG 预览。",
            ));
        }
    };
    if length > MAX_PREVIEW_BYTES {
        return Err(DomainError::new(
            ErrorCode::MediaUnreadable,
            "内嵌预览超过安全读取上限。",
        ));
    }
    file.seek(SeekFrom::Start(offset)).map_err(media_io_error)?;
    let mut bytes = vec![
        0;
        usize::try_from(length).map_err(|_| {
            DomainError::new(ErrorCode::MediaUnreadable, "预览尺寸无法表示。")
        })?
    ];
    file.read_exact(&mut bytes).map_err(media_io_error)?;
    if bytes.len() < 4 || !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(DomainError::new(
            ErrorCode::MediaUnreadable,
            "内嵌预览不是完整 JPEG。",
        ));
    }
    Ok(bytes)
}

fn media_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "无法只读解析照片元数据。")
        .with_diagnostic(error.to_string())
}
