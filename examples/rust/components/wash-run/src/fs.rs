use crate::wasi::filesystem::types::{Descriptor, DescriptorType, ErrorCode};

#[derive(serde::Serialize)]
pub struct PreopenedDir {
    content: Vec<String>,
}

impl TryInto<PreopenedDir> for Descriptor {
    type Error = ErrorCode;

    fn try_into(self) -> Result<PreopenedDir, Self::Error> {
        let mut content: Vec<String> = Vec::new();

        let fs_type = self.get_type()?;
        if fs_type != DescriptorType::Directory {
            return Ok(PreopenedDir { content });
        }

        let dir_stream = self.read_directory()?;
        while let Some(dir_entry) = dir_stream.read_directory_entry()? {
            content.push(dir_entry.name.clone());
        }

        Ok(PreopenedDir {
            content,
        })
    }
}
