//! 机器狗本体 SN（= 业务平台的设备编号）读取。
//!
//! 现场读法（本体上是 arm64 Ubuntu，`/dev/mmcblk0p5` 是写 SN 的分区）：
//!
//! ```text
//! $ hexdump -C /dev/mmcblk0p5 | sed -n '3s/.*|\(.*\)|.*/\1/p' | tr -d ' '
//! 0202222672400230
//! ```
//!
//! 注意 `hexdump -C` 会把**连续相同的行（开头全 0）折叠成 `*`**，所以命令里的
//! 「第 3 行」在现场这个分区上其实是**偏移 0x200** 那一行（前 512 字节全 0）：
//!
//! ```text
//! 00000000  00 00 ... 00                    |................|
//! *
//! 00000200  30 32 30 32 ... 32 33 30        |0202222672400230|
//! ```
//!
//! 所以这里不依赖行号，而是扫描前若干字节里 `hexdump -C` 的每一行（16 字节），
//! 取第一条「ASCII 全部为字母数字」的行；16 位十六进制的行优先命中。
//!
//! 为什么在代码里算而不是写配置：设备编号是资产级身份（一台机器狗一个），
//! 手工抄写容易抄错（现场就写错过一次），换机器狗也要改配置。分区里的值不会错。

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 默认 SN 分区（本体 eMMC）
pub const DEFAULT_SN_PATH: &str = "/dev/mmcblk0p5";
/// `hexdump -C` 一行的字节数
const LINE_BYTES: usize = 16;
/// 读取长度：**必须是逻辑块大小的整数倍**（512/4096 都行）。
///
/// 块设备上 `read(2)` 长度不是逻辑块大小整数倍会返回 `EINVAL`；
/// 这里读 4KB，足够覆盖 SN（现场在 0x200）也兼容不同固件。
const READ_BYTES: usize = 4096;
/// SN 最短长度：再短说明读到的是空白或垃圾
const MIN_SN_LEN: usize = 8;
/// SN 最长长度（防止把一整行配置误当 SN）
const MAX_SN_LEN: usize = 32;

/// 读取失败的原因（带路径，便于现场排障）
#[derive(Debug)]
pub struct SnError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for SnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "读取设备 SN 失败（{}）：{}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for SnError {}

/// 从分区文件读出设备编号。
///
/// 需要读裸块设备，通常要 root（systemd 单元以 root 运行，正常场景没问题）。
pub fn detect(path: &Path) -> Result<String, SnError> {
    let mut file = File::open(path).map_err(|error| SnError {
        path: path.to_path_buf(),
        reason: format!("打开失败: {error}"),
    })?;
    let mut buffer = [0u8; READ_BYTES];
    let read = file.read(&mut buffer).map_err(|error| SnError {
        path: path.to_path_buf(),
        reason: format!("读取失败: {error}"),
    })?;
    sn_from_bytes(&buffer[..read]).ok_or_else(|| SnError {
        path: path.to_path_buf(),
        reason: "分区前 4KB 里找不到字母数字组成的 SN（分区格式与预期不符）".to_string(),
    })
}

/// 从分区前若干字节里取出 SN（纯函数，便于单测）。
///
/// 逐条 16 字节行（等价 `hexdump -C` 的 ASCII 列）扫描，去掉空白与非可见字符；
/// 全字母数字才算候选，16 位十六进制的候选最优先（现场 SN 就是这个形态）。
pub fn sn_from_bytes(bytes: &[u8]) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in bytes.chunks_exact(LINE_BYTES) {
        let text: String = line
            .iter()
            .map(|byte| {
                if byte.is_ascii_graphic() {
                    char::from(*byte)
                } else {
                    ' '
                }
            })
            .collect();
        let candidate: String = text.split_whitespace().collect();
        if candidate.len() < MIN_SN_LEN || candidate.len() > MAX_SN_LEN {
            continue;
        }
        if !candidate.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            continue;
        }
        // 现场 SN = 16 位十六进制，优先返回
        if candidate.len() == 16 && candidate.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Some(candidate);
        }
        if fallback.is_none() {
            fallback = Some(candidate);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 造一个「分区镜像」：SN 写在指定的 16 字节行（现场在 0x200）
    fn partition_at(sn: &str, offset: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; READ_BYTES];
        let body = format!("{sn:<LINE_BYTES$}");
        bytes[offset..offset + LINE_BYTES].copy_from_slice(body.as_bytes());
        bytes
    }

    /// 现场布局：前 512 字节全 0，SN 在 0x200
    fn partition(sn: &str) -> Vec<u8> {
        partition_at(sn, 0x200)
    }

    #[test]
    fn reads_sn_from_real_partition_layout() {
        assert_eq!(
            sn_from_bytes(&partition("0202222672400230")).as_deref(),
            Some("0202222672400230")
        );
    }

    #[test]
    fn finds_sn_wherever_the_line_sits() {
        // 不依赖行号：0x20 / 0x200 / 0x800 都能取到
        for offset in [0x20usize, 0x200, 0x800] {
            assert_eq!(
                sn_from_bytes(&partition_at("0202222672400230", offset)).as_deref(),
                Some("0202222672400230"),
                "offset 0x{offset:x}"
            );
        }
    }

    #[test]
    fn prefers_16_hex_digits_over_earlier_text() {
        let mut bytes = partition("0202222672400230");
        // 前面先放一行普通文字（≥8 位字母数字），不应抢走 SN
        let text = format!("{: <LINE_BYTES$}", "MAPDATA1");
        bytes[0x40..0x50].copy_from_slice(text.as_bytes());
        assert_eq!(sn_from_bytes(&bytes).as_deref(), Some("0202222672400230"));
    }

    #[test]
    fn trims_padding_and_ignores_short_values() {
        // 16 字符位的值后面是空格，等价 `tr -d ' '`
        assert_eq!(
            sn_from_bytes(&partition("SN12345678")).as_deref(),
            Some("SN12345678")
        );
        assert_eq!(sn_from_bytes(&partition("   ")), None);
        assert_eq!(sn_from_bytes(&partition("1234")), None);
    }

    #[test]
    fn rejects_non_alphanumeric_garbage() {
        assert_eq!(sn_from_bytes(&partition("0202-2226/724")), None);
    }

    #[test]
    fn rejects_too_short_partition() {
        assert_eq!(sn_from_bytes(&[0u8; 0x2f]), None, "不足完整一行应判失败");
    }

    #[test]
    fn detect_reads_file_and_reports_path_on_failure() {
        let path = std::env::temp_dir().join(format!(
            "robodog-sn-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::File::create(&path).expect("创建临时分区失败");
        file.write_all(&partition("0202222ABCDE001"))
            .expect("写入失败");
        drop(file);

        assert_eq!(detect(&path).unwrap(), "0202222ABCDE001");
        std::fs::remove_file(&path).ok();

        let error = detect(Path::new("/dev/definitely-not-a-partition")).unwrap_err();
        assert!(error.to_string().contains("读取设备 SN 失败"));
    }
}
