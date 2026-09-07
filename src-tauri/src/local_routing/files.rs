//! Bounded, private configuration I/O. Never use these helpers for auth.json writes.
use crate::error::{AppError, AppResult};
use std::{fs, io::Write, path::Path};

pub const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

pub fn reject_links(path: &Path) -> AppResult<()> {
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(meta) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if meta.file_type().is_symlink() || reparse {
                    return Err(AppError::Conflict(
                        "路由配置路径包含链接或重解析点，未进行写入".into(),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AppError::Io("无法核验路由配置路径".into())),
        }
    }
    Ok(())
}

pub fn read_optional(path: &Path) -> AppResult<Option<String>> {
    reject_links(path)?;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(AppError::Io("无法读取路由或 Codex 配置文件".into())),
    };
    if !file.metadata()?.is_file() {
        return Err(AppError::Io("路由配置不是普通文件".into()));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(AppError::Io("路由或 Codex 配置超过 2 MiB 限制".into()));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| AppError::Config("配置文件不是有效 UTF-8".into()))
}

pub fn private_directory(path: &Path) -> AppResult<()> {
    reject_links(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn atomic_write(path: &Path, content: &str) -> AppResult<()> {
    if content.len() as u64 > MAX_CONFIG_BYTES {
        return Err(AppError::Io("配置内容超过大小限制".into()));
    }
    reject_links(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io("配置缺少父目录".into()))?;
    if !parent.is_dir() {
        return Err(AppError::Io("配置目录不存在".into()));
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    reject_links(path)?;
    temp.persist(path)
        .map_err(|_| AppError::Io("原子写入路由配置失败，原文件未主动删除".into()))?;
    Ok(())
}

pub fn compare_and_write(path: &Path, before: &Option<String>, after: &str) -> AppResult<()> {
    if &read_optional(path)? != before {
        return Err(AppError::Conflict(
            "Codex 配置刚被其他程序修改，请刷新后重试；未覆盖其修改".into(),
        ));
    }
    atomic_write(path, after)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn explicit_symlink_ancestors_and_files_remain_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let target = root.join("real");
        fs::create_dir(&target).unwrap();
        let config = target.join("config.toml");
        fs::write(&config, "original").unwrap();
        let alias = root.join("alias");
        symlink(&target, &alias).unwrap();
        assert!(read_optional(&alias.join("config.toml")).is_err());
        assert!(atomic_write(&alias.join("config.toml"), "overwritten").is_err());
        let file_alias = root.join("config-link.toml");
        symlink(&config, &file_alias).unwrap();
        assert!(read_optional(&file_alias).is_err());
        assert!(atomic_write(&file_alias, "overwritten").is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), "original");
    }
}
