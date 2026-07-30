use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{DomainError, ErrorCode};

macro_rules! strong_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            pub fn as_bytes(&self) -> &[u8; 16] {
                self.0.as_bytes()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidId, "请求中的对象标识无效。")
                        .with_diagnostic(format!("{}: {error}", stringify!($name)))
                })
            }
        }
    };
}

strong_id!(AssetId);
strong_id!(AttemptId);
strong_id!(CacheEntryId);
strong_id!(CacheEvictionIntentId);
strong_id!(CacheRootCreateIntentId);
strong_id!(DirectorySnapshotId);
strong_id!(DeletionMarkId);
strong_id!(FileSnapshotId);
strong_id!(JobId);
strong_id!(LibraryId);
strong_id!(OperationId);
strong_id!(PhotoId);
strong_id!(ReportId);
strong_id!(ReservationId);
strong_id!(RootBindingId);
strong_id!(RootId);
strong_id!(ScanRunId);
strong_id!(TaskId);
strong_id!(ToolId);
strong_id!(VolumeId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_do_not_cross_assign_types() {
        let root = RootId::new();
        let asset = AssetId::from_uuid(root.as_uuid());

        assert_eq!(root.as_bytes(), asset.as_bytes());
        assert_eq!(std::mem::size_of::<RootId>(), 16);
        assert_eq!(std::mem::size_of::<AssetId>(), 16);
    }

    #[test]
    fn malformed_external_id_becomes_domain_error() {
        let error = "not-an-id".parse::<PhotoId>().unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidId);
        assert!(!error.user_message.contains("UUID"));
    }
}
