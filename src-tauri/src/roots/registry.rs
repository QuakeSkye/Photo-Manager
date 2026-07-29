use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::domain::{DomainError, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeIdentityKey(String);

impl VolumeIdentityKey {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "无法可靠识别候选卷。",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Default)]
pub struct RootRegistry {
    import_source_deny_write: HashSet<VolumeIdentityKey>,
    application_write_volumes: HashSet<VolumeIdentityKey>,
}

impl RootRegistry {
    pub fn register_import_source(
        &mut self,
        volume: VolumeIdentityKey,
    ) -> Result<bool, DomainError> {
        if self.application_write_volumes.contains(&volume) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "该卷承载应用必须写入的位置，不能作为内存卡来源。",
            ));
        }

        Ok(self.import_source_deny_write.insert(volume))
    }

    pub fn register_application_write_volume(
        &mut self,
        volume: VolumeIdentityKey,
    ) -> Result<bool, DomainError> {
        if self.import_source_deny_write.contains(&volume) {
            return Err(DomainError::new(
                ErrorCode::VolumeWriteDenied,
                "该卷曾作为内存卡来源，应用永久拒绝向该卷写入。",
            ));
        }

        Ok(self.application_write_volumes.insert(volume))
    }

    pub fn is_write_denied(&self, volume: &VolumeIdentityKey) -> bool {
        self.import_source_deny_write.contains(volume)
    }

    pub fn import_source_count(&self) -> usize {
        self.import_source_deny_write.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(value: &str) -> VolumeIdentityKey {
        VolumeIdentityKey::parse(value).unwrap()
    }

    #[test]
    fn deny_write_is_idempotent_and_has_no_removal_path() {
        let mut registry = RootRegistry::default();
        let source = volume("strong:v1:source-card");

        assert!(registry.register_import_source(source.clone()).unwrap());
        assert!(!registry.register_import_source(source.clone()).unwrap());
        assert!(registry.is_write_denied(&source));
        assert_eq!(registry.import_source_count(), 1);
    }

    #[test]
    fn import_source_rejects_existing_application_write_volume() {
        let mut registry = RootRegistry::default();
        let app_data = volume("strong:v1:app-data");

        registry
            .register_application_write_volume(app_data.clone())
            .unwrap();

        assert!(registry.register_import_source(app_data).is_err());
    }

    #[test]
    fn protected_source_volume_can_never_become_write_volume() {
        let mut registry = RootRegistry::default();
        let source = volume("strong:v1:source-card");

        registry.register_import_source(source.clone()).unwrap();

        let error = registry
            .register_application_write_volume(source)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::VolumeWriteDenied);
    }
}
