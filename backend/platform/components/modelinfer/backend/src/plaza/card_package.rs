//! 本地模型卡片包：归档格式识别 / 安全解压 / 模型定义包定位 / 重打包
//!
//! 输入是用户上传的 `tar.*` 归档（见 `specs/docs/design/modelinfer/model-package.md` §1/§4）：
//!
//! - **外层离线安装包** `model_<id>_<version>.tar`：含 `model.tar.gz` 与可选 `assets/`；
//! - **模型定义包** `model.tar.gz`：解压后根即 `manifest.yaml`（可能再嵌套一层目录）。
//!
//! 本模块只负责"格式识别 / 安全解压 / 定位 / 重打包"（纯函数 + IO 边界，便于用临时目录单测），
//! 不触碰数据库；登记落库见 [`crate::plaza::register_local_card_package`]。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use xz2::read::XzDecoder;

use crate::error::{ModelinferError, codes};

/// gzip 魔数（`1f 8b`）
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];
/// xz 魔数（`fd 37 7a 58 5a 00`）
const XZ_MAGIC: &[u8] = &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00];
/// bzip2 魔数（`BZh`，暂不支持）
const BZIP2_MAGIC: &[u8] = &[0x42, 0x5a, 0x68];
/// zstd 魔数（`28 b5 2f fd`，暂不支持）
const ZSTD_MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

/// 支持的归档格式（按魔数识别，不依赖扩展名）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// 未压缩 tar
    Tar,
    /// `tar.gz` / `tgz`
    TarGz,
    /// `tar.xz`
    TarXz,
}

/// 模型定义包来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardSource {
    /// 归档中的 `model.tar.gz`
    Package(PathBuf),
    /// 含 `manifest.yaml` 的卡片目录（需重打包为 `model.tar.gz`）
    Dir(PathBuf),
}

/// 按魔数识别归档格式；不支持的压缩格式 / 空输入返回 `None`
pub fn detect_format(head: &[u8]) -> Option<ArchiveFormat> {
    if head.starts_with(GZIP_MAGIC) {
        Some(ArchiveFormat::TarGz)
    } else if head.starts_with(XZ_MAGIC) {
        Some(ArchiveFormat::TarXz)
    } else if head.is_empty() {
        None
    } else if head.starts_with(BZIP2_MAGIC) || head.starts_with(ZSTD_MAGIC) {
        // bzip2 / zstd 暂不支持（明确报错而非误判为 plain tar）
        None
    } else {
        Some(ArchiveFormat::Tar)
    }
}

/// 读取文件头部若干字节（用于魔数识别）
fn read_head(path: &Path, n: usize) -> Result<Vec<u8>, ModelinferError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("打开文件失败 {}: {e}", path.display())))?;
    let mut buf = vec![0u8; n];
    let read = file
        .read(&mut buf)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("读取文件失败: {e}")))?;
    buf.truncate(read);
    Ok(buf)
}

fn pack_err(msg: impl Into<String>) -> ModelinferError {
    ModelinferError::new(codes::CARD_PACKAGE_INVALID, msg)
}

/// 条目路径是否含危险段（`..` / 绝对路径 / Windows 前缀）
pub fn unsafe_entry_path(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// 计算条目在解压根下的目标路径（拒绝越界）
fn entry_dest(root: &Path, entry_path: &Path) -> Result<PathBuf, ModelinferError> {
    if unsafe_entry_path(entry_path) {
        return Err(pack_err(format!(
            "tar 条目含非法路径段: '{}'（禁止 '..' / 绝对路径）",
            entry_path.display()
        )));
    }
    let target = root.join(entry_path);
    // 二次防护：join 后目标必须仍以解压根开头（不依赖 canonicalize）
    if !target.starts_with(root) {
        return Err(pack_err(format!(
            "tar 条目越出解压根: '{}'",
            entry_path.display()
        )));
    }
    Ok(target)
}

/// 逐条目安全解压 tar 流到 `dest`：
/// - 目录 → `create_dir_all`；普通文件 → `entry.unpack`；
/// - symlink / hardlink / 设备 / fifo 等非普通文件条目 **跳过**（防符号链接逃逸）；
/// - 任何含 `..` / 绝对路径的条目 → 20724。
pub fn unpack_tar<R: Read>(reader: R, dest: &Path) -> Result<(), ModelinferError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建解压目录失败: {e}")))?;
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| pack_err(format!("遍历归档失败: {e}")))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| pack_err(format!("读取 tar 条目失败: {e}")))?;
        let raw_path = entry
            .path()
            .map_err(|e| pack_err(format!("解析 tar 条目路径失败: {e}")))?
            .to_path_buf();
        let entry_type = entry.header().entry_type();

        if entry_type.is_dir() {
            let target = entry_dest(dest, &raw_path)?;
            std::fs::create_dir_all(&target).map_err(|e| {
                ModelinferError::new(codes::IO_FAILED, format!("创建条目目录失败 {}: {e}", target.display()))
            })?;
            continue;
        }
        if !entry_type.is_file() {
            tracing::warn!(path = %raw_path.display(), "跳过非普通文件 tar 条目（symlink/hardlink/设备等）");
            continue;
        }

        let target = entry_dest(dest, &raw_path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ModelinferError::new(codes::IO_FAILED, format!("创建条目父目录失败 {}: {e}", parent.display()))
            })?;
        }
        entry.unpack(&target).map_err(|e| {
            pack_err(format!("解压条目失败 '{}': {e}", raw_path.display()))
        })?;
    }
    Ok(())
}

/// 按魔数识别并解压归档到 `dest`，返回识别到的格式
pub fn unpack_archive(path: &Path, dest: &Path) -> Result<ArchiveFormat, ModelinferError> {
    let head = read_head(path, 8)?;
    let format = detect_format(&head).ok_or_else(|| {
        pack_err("不支持的归档格式（仅支持 tar / tar.gz / tgz / tar.xz）")
    })?;
    let file = std::fs::File::open(path)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("打开归档失败 {}: {e}", path.display())))?;
    match format {
        ArchiveFormat::TarGz => unpack_tar(GzDecoder::new(file), dest),
        ArchiveFormat::TarXz => unpack_tar(XzDecoder::new(file), dest),
        ArchiveFormat::Tar => unpack_tar(file, dest),
    }?;
    Ok(format)
}

/// 在 `root` 下查找名为 `name` 的文件（广度优先，返回最浅一层）
fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(root.to_path_buf());
    while let Some(dir) = queue.pop_front() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if entry.file_name().to_string_lossy() == name {
                    return Some(path);
                }
            } else if path.is_dir() {
                subdirs.push(path);
            }
        }
        // 同层全部检查完再进入下一层（保证最浅优先）
        queue.extend(subdirs);
    }
    None
}

/// 递归定位模型定义包：
/// 1. 优先名为 `model.tar.gz` 的文件（最浅一层）；
/// 2. 否则取最浅一层含 `manifest.yaml` 的目录（需重打包）；
/// 3. 都没有 → `None`。
pub fn locate_card_source(root: &Path) -> Option<CardSource> {
    if let Some(pkg) = find_file(root, "model.tar.gz") {
        return Some(CardSource::Package(pkg));
    }
    find_file(root, "manifest.yaml")
        .and_then(|m| m.parent().map(|d| CardSource::Dir(d.to_path_buf())))
}

/// 将卡片目录内容递归打包为 `model.tar.gz`（解压后根即 `manifest.yaml`，无嵌套目录）
pub fn pack_card_dir(dir: &Path, dest: &Path) -> Result<(), ModelinferError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ModelinferError::new(codes::IO_FAILED, format!("创建打包目录失败: {e}"))
        })?;
    }
    let file = std::fs::File::create(dest)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建模型包失败 {}: {e}", dest.display())))?;
    let gz = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(gz);
    builder.follow_symlinks(false);
    append_dir_recursive(&mut builder, dir, dir)?;
    let gz = builder
        .into_inner()
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("写入模型包失败: {e}")))?;
    gz.finish()
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("压缩模型包失败: {e}")))?;
    Ok(())
}

fn append_dir_recursive<W: Write>(
    builder: &mut tar::Builder<W>,
    root: &Path,
    dir: &Path,
) -> Result<(), ModelinferError> {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
        Err(e) => {
            return Err(ModelinferError::new(
                codes::IO_FAILED,
                format!("读取目录失败 {}: {e}", dir.display()),
            ));
        }
    };
    paths.sort();
    for path in paths {
        let rel = path
            .strip_prefix(root)
            .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("计算相对路径失败: {e}")))?;
        if path.is_dir() {
            builder
                .append_dir(rel, &path)
                .map_err(|e| pack_err(format!("追加目录条目失败 {}: {e}", rel.display())))?;
            append_dir_recursive(builder, root, &path)?;
        } else if path.is_file() {
            builder
                .append_path_with_name(&path, rel)
                .map_err(|e| pack_err(format!("追加文件条目失败 {}: {e}", rel.display())))?;
        }
        // symlink 等非普通文件跳过
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造内存 tar 归档（`compression`: "none" / "gz" / "xz"）
    fn build_tar(files: &[(&str, &[u8])], compression: &str) -> Vec<u8> {
        let raw = {
            let mut buf = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut buf);
                for (name, content) in files {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(content.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    builder.append_data(&mut header, name, *content).unwrap();
                }
                builder.finish().unwrap();
            }
            buf
        };
        match compression {
            "gz" => {
                let mut gz = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
                gz.write_all(&raw).unwrap();
                gz.finish().unwrap()
            }
            "xz" => {
                let mut xz = xz2::write::XzEncoder::new(Vec::new(), 6);
                xz.write_all(&raw).unwrap();
                xz.finish().unwrap()
            }
            _ => raw,
        }
    }

    #[test]
    fn detect_format_by_magic() {
        assert_eq!(detect_format(&[0x1f, 0x8b, 0x08]), Some(ArchiveFormat::TarGz));
        assert_eq!(
            detect_format(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 0x00]),
            Some(ArchiveFormat::TarXz)
        );
        assert_eq!(detect_format(b"ustar\x00"), Some(ArchiveFormat::Tar));
        // 不支持的压缩格式
        assert_eq!(detect_format(b"BZh9"), None);
        assert_eq!(detect_format(&[0x28, 0xb5, 0x2f, 0xfd]), None);
        assert_eq!(detect_format(&[]), None);
    }

    #[test]
    fn unsafe_paths_rejected() {
        assert!(unsafe_entry_path(Path::new("../evil")));
        assert!(unsafe_entry_path(Path::new("/etc/passwd")));
        assert!(!unsafe_entry_path(Path::new("manifest.yaml")));
        assert!(!unsafe_entry_path(Path::new("scripts/start.sh")));
        let root = Path::new("/tmp/card-root");
        assert!(entry_dest(root, Path::new("../evil")).is_err());
        assert_eq!(entry_dest(root, Path::new("manifest.yaml")).unwrap(), root.join("manifest.yaml"));
    }

    #[test]
    fn unpack_rejects_traversal_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        // 手工构造含 `../evil` 条目的 tar（tar crate 的 append_data 会拒绝该路径，故直接写 name 字段）
        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            let content = b"x";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            {
                let name = b"../evil";
                let old = header.as_old_mut();
                old.name[..name.len()].copy_from_slice(name);
            }
            header.set_cksum();
            builder.append(&header, &content[..]).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&buf).unwrap();
        let bytes = gz.finish().unwrap();

        let dest = tmp.path().join("out");
        let pkg = write_tmp(tmp.path(), "bad.tar.gz", &bytes);
        let err = unpack_archive(&pkg, &dest).unwrap_err();
        assert_eq!(err.code, codes::CARD_PACKAGE_INVALID);
        assert!(!tmp.path().join("evil").exists());
    }

    fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn unpack_plain_gz_and_xz() {
        let tmp = tempfile::TempDir::new().unwrap();
        for (name, comp) in [("a.tar", "none"), ("a.tar.gz", "gz"), ("a.tar.xz", "xz")] {
            let bytes = build_tar(&[("manifest.yaml", b"id: a.b\nversion: 1.0.0\nmodel: o/r\n")], comp);
            let pkg = write_tmp(tmp.path(), name, &bytes);
            let dest = tmp.path().join(format!("out-{comp}"));
            unpack_archive(&pkg, &dest).unwrap();
            assert!(dest.join("manifest.yaml").exists(), "{name} 应解压出 manifest.yaml");
        }
    }

    #[test]
    fn locate_prefers_model_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // 外层结构：model.tar.gz + assets/
        std::fs::create_dir_all(root.join("assets/amd64")).unwrap();
        std::fs::write(root.join("assets/amd64/x.tar.gz"), b"x").unwrap();
        std::fs::write(root.join("model.tar.gz"), b"pkg").unwrap();
        assert_eq!(
            locate_card_source(root),
            Some(CardSource::Package(root.join("model.tar.gz")))
        );
    }

    #[test]
    fn locate_falls_back_to_manifest_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("card/scripts")).unwrap();
        std::fs::write(root.join("card/manifest.yaml"), b"id: a.b\n").unwrap();
        std::fs::write(root.join("card/scripts/start.sh"), b"#!/bin/sh\n").unwrap();
        assert_eq!(
            locate_card_source(root),
            Some(CardSource::Dir(root.join("card")))
        );
    }

    #[test]
    fn locate_none_when_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(locate_card_source(tmp.path()), None);
    }

    #[test]
    fn pack_and_unpack_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let card = tmp.path().join("card");
        std::fs::create_dir_all(card.join("scripts")).unwrap();
        std::fs::write(card.join("manifest.yaml"), b"id: a.b\nversion: 1.0.0\nmodel: o/r\n").unwrap();
        std::fs::write(card.join("scripts/start.sh"), b"#!/bin/sh\necho hi\n").unwrap();

        let pkg = tmp.path().join("model.tar.gz");
        pack_card_dir(&card, &pkg).unwrap();
        let out = tmp.path().join("out");
        unpack_archive(&pkg, &out).unwrap();
        assert!(out.join("manifest.yaml").exists());
        assert!(out.join("scripts/start.sh").exists());
        // 打包使用 gzip 魔数
        let head = read_head(&pkg, 2).unwrap();
        assert_eq!(detect_format(&head), Some(ArchiveFormat::TarGz));
    }

    #[test]
    fn unsupported_format_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pkg = write_tmp(tmp.path(), "a.tar.bz2", b"BZh91AY&SY");
        let err = unpack_archive(&pkg, &tmp.path().join("out")).unwrap_err();
        assert_eq!(err.code, codes::CARD_PACKAGE_INVALID);
    }
}