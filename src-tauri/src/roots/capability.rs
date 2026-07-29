use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Enumerate,
    ReadOpen,
    ReadMetadata,
    ReadHash,
    CreateNew,
    PublishNoReplace,
    Recycle,
    CacheWrite,
    AppDataWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    pub const NONE: Self = Self(0);

    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, capability: Capability) -> bool {
        (self.0 & capability.bit()) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl Capability {
    const fn bit(self) -> u16 {
        match self {
            Self::Enumerate => 1 << 0,
            Self::ReadOpen => 1 << 1,
            Self::ReadMetadata => 1 << 2,
            Self::ReadHash => 1 << 3,
            Self::CreateNew => 1 << 4,
            Self::PublishNoReplace => 1 << 5,
            Self::Recycle => 1 << 6,
            Self::CacheWrite => 1 << 7,
            Self::AppDataWrite => 1 << 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    FujiLibrary,
    ImportSource,
    AppDataRoot,
    CacheRoot,
}

impl RootKind {
    pub const fn allowed_capabilities(self) -> CapabilitySet {
        let read = CapabilitySet::from_bits(
            Capability::Enumerate.bit()
                | Capability::ReadOpen.bit()
                | Capability::ReadMetadata.bit()
                | Capability::ReadHash.bit(),
        );

        match self {
            Self::FujiLibrary => read.union(CapabilitySet::from_bits(
                Capability::CreateNew.bit()
                    | Capability::PublishNoReplace.bit()
                    | Capability::Recycle.bit(),
            )),
            Self::ImportSource => read,
            Self::AppDataRoot => {
                read.union(CapabilitySet::from_bits(Capability::AppDataWrite.bit()))
            }
            Self::CacheRoot => read.union(CapabilitySet::from_bits(Capability::CacheWrite.bit())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_source_is_permanently_read_only_by_capability() {
        let capabilities = RootKind::ImportSource.allowed_capabilities();

        assert!(capabilities.contains(Capability::ReadOpen));
        assert!(capabilities.contains(Capability::ReadHash));
        assert!(!capabilities.contains(Capability::CreateNew));
        assert!(!capabilities.contains(Capability::PublishNoReplace));
        assert!(!capabilities.contains(Capability::Recycle));
        assert!(!capabilities.contains(Capability::CacheWrite));
        assert!(!capabilities.contains(Capability::AppDataWrite));
    }
}
