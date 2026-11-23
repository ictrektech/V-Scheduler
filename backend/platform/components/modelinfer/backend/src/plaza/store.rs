//! 模型广场本地存储：路径、sha256、tar.gz 解压（IO 边界）

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{ModelinferError, codes};
use crate::plaza::model_index::{ManifestMeta, parse_manifest};

/// 卡片本地目录：`<plaza_dir>/<card_id>/<version>`
pub fn card_dir(plaza_dir: &Path, card_id: &str, version: &str) -> PathBuf {
    plaza_dir.join(card_id).join(version)
}

/// model.tar.gz 的本地路径
pub fn package_path(card_dir: &Path) -> PathBuf {
    card_dir.join("model.tar.gz")
}

/// 计算字节的 sha256（十六进制小写）
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 计算文件的 sha256（流式，避免大文件 OOM）
pub fn sha256_file(path: &Path) -> Result<String, ModelinferError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("打开文件失败 {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("读取文件失败: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 解压 tar.gz 到目标目录（目录会被创建；不保留文件权限位之外的特殊属性）
pub fn extract_tar_gz(tar_path: &Path, dest: &Path) -> Result<(), ModelinferError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建解压目录失败: {e}")))?;
    let file = std::fs::File::open(tar_path)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("打开模型包失败: {e}")))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest)
        .map_err(|e| ModelinferError::new(codes::MANIFEST_INVALID, format!("解压模型包失败: {e}")))?;
    Ok(())
}

/// 读取目录下的 manifest.yaml 并解析
pub fn read_manifest(dir: &Path) -> Result<ManifestMeta, ModelinferError> {
    let path = dir.join("manifest.yaml");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        ModelinferError::new(codes::MANIFEST_INVALID, format!("读取 manifest 失败 {}: {e}", path.display()))
    })?;
    parse_manifest(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            for (name, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, content.as_bytes()).unwrap();
            }
            builder.finish().unwrap();
        }
        gz.write_all(&[]).ok();
        gz.finish().unwrap()
    }

    #[test]
    fn sha256_known() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn extract_and_read_manifest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let card = tmp.path().join("card");
        std::fs::create_dir_all(&card).unwrap();
        let bytes = make_tar_gz(&[(
            "manifest.yaml",
            "id: a.b\nversion: 1.0.0\nmodel: org/repo\n",
        )]);
        let pkg = package_path(&card);
        std::fs::write(&pkg, &bytes).unwrap();
        let src = card.join("src");
        extract_tar_gz(&pkg, &src).unwrap();
        let m = read_manifest(&src).unwrap();
        assert_eq!(m.model, "org/repo");
    }
}