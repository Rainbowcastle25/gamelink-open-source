use crate::host;
use gamelink_core::storage;
use serde_json::json;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(crate) fn config_dir() -> Result<PathBuf, String> {
    let paths = host::call(json!({"method":"paths"}))?;
    paths["configDir"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "could not locate configuration folder".into())
}

pub(crate) fn read_optional(dir: &Path, file_name: &str) -> Result<Option<Vec<u8>>, String> {
    let path = dir.join(file_name);
    match storage::read_bounded(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error)
            if error.kind() == ErrorKind::NotFound
                && matches!(path.symlink_metadata(), Err(e) if e.kind() == ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(_) => Err(format!(
            "Could not read {file_name}. Check its permissions or restore it in the configuration folder, then reload."
        )),
    }
}
