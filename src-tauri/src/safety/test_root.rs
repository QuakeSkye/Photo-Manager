use std::{
    fs,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::domain::{DomainError, ErrorCode};

const MARKER_FILE: &str = ".fuji-photo-manager-test-root-v1";
const DIRECTORY_PREFIX: &str = "fpm-generated-test-root-";

#[derive(Debug)]
pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn create_in(parent: &Path) -> Result<Self, DomainError> {
        let directory = parent.join(format!("{DIRECTORY_PREFIX}{}", Uuid::new_v4()));
        fs::create_dir(&directory).map_err(|error| {
            DomainError::new(ErrorCode::TestRootRequired, "无法创建可丢弃测试 Root。")
                .with_diagnostic(error.to_string())
        })?;
        fs::write(directory.join(MARKER_FILE), b"generated-test-root-v1\n").map_err(|error| {
            DomainError::new(ErrorCode::TestRootRequired, "无法标记可丢弃测试 Root。")
                .with_diagnostic(error.to_string())
        })?;

        Ok(Self { path: directory })
    }

    pub fn verify(path: &Path) -> Result<Self, DomainError> {
        let directory_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let marker = path.join(MARKER_FILE);
        let marker_contents = fs::read(&marker).map_err(|_| {
            DomainError::new(
                ErrorCode::TestRootRequired,
                "破坏性测试只允许使用应用生成的可丢弃测试 Root。",
            )
        })?;

        if !directory_name.starts_with(DIRECTORY_PREFIX)
            || marker_contents != b"generated-test-root-v1\n"
        {
            return Err(DomainError::new(
                ErrorCode::TestRootRequired,
                "破坏性测试只允许使用应用生成的可丢弃测试 Root。",
            ));
        }

        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_root_is_accepted() {
        let parent = tempfile::tempdir().unwrap();
        let generated = TestRoot::create_in(parent.path()).unwrap();

        let verified = TestRoot::verify(generated.path()).unwrap();
        assert_eq!(verified.path(), generated.path());
    }

    #[test]
    fn arbitrary_directory_is_rejected() {
        let arbitrary = tempfile::tempdir().unwrap();

        let error = TestRoot::verify(arbitrary.path()).unwrap_err();
        assert_eq!(error.code, ErrorCode::TestRootRequired);
    }

    #[test]
    fn copied_marker_without_generated_name_is_rejected() {
        let arbitrary = tempfile::tempdir().unwrap();
        fs::write(
            arbitrary.path().join(MARKER_FILE),
            b"generated-test-root-v1\n",
        )
        .unwrap();

        assert!(TestRoot::verify(arbitrary.path()).is_err());
    }
}
