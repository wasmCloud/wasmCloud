use std::fmt::{Display, Formatter};

use crate::fs::ReportDescriptorError::{FsError, NotADirectory};
use crate::wasi::filesystem::types::{Descriptor, DescriptorFlags, DescriptorType, ErrorCode};

#[derive(serde::Serialize, Default)]
pub struct PreopenedDir {
    content: Option<Vec<String>>,
    mutate_dir: bool,
    read_dir: bool,
}

impl PreopenedDir {
    pub fn report_descriptor(descriptor: Descriptor) -> Result<PreopenedDir, ReportDescriptorError> {
        let mut result = PreopenedDir::default();
        
        let fs_type = descriptor.get_type()?;
        if fs_type != DescriptorType::Directory {
            return Err(NotADirectory(fs_type));
        }
        
        result.mutate_dir = descriptor.get_flags()?.contains(DescriptorFlags::MUTATE_DIRECTORY);
        result.read_dir = descriptor.get_flags()?.contains(DescriptorFlags::READ);
        
        if result.read_dir {
            let mut content = Vec::new();
            let dir_stream = descriptor.read_directory()?;
            while let Some(dir_entry) = dir_stream.read_directory_entry()? {
                content.push(dir_entry.name.clone());
            }
            result.content = Some(content);
        }
        
        Ok(result)
    }
}

#[derive(Debug)]
pub enum ReportDescriptorError {
    FsError(ErrorCode),
    NotADirectory(DescriptorType),
}

impl From<ErrorCode> for ReportDescriptorError {
    fn from(value: ErrorCode) -> Self {
        FsError(value)
    }
}

impl Display for ReportDescriptorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FsError(err_code) => write!(f, "filesystem error: {}", err_code),
            NotADirectory(desc_type) => write!(f, "descriptor is not a directory: got {:?}", desc_type)
        }
    }
}
