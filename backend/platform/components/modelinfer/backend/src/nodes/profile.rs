use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Auto,
    #[serde(alias = "thor", alias = "jetson-thor")]
    ThorSpark,
    #[serde(alias = "l4t")]
    JetsonOrin,
    ArmCpu,
    AmdCpu,
    AmdCuda,
    ArmCuda,
    SophonBm1688,
}

impl Profile {
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::ThorSpark => Some("thor-spark"),
            Self::JetsonOrin => Some("jetson-orin"),
            Self::ArmCpu => Some("arm-cpu"),
            Self::AmdCpu => Some("amd-cpu"),
            Self::AmdCuda => Some("amd-cuda"),
            Self::ArmCuda => Some("arm-cuda"),
            Self::SophonBm1688 => Some("sophon-bm1688"),
        }
    }
}

pub fn detect(arch: &str, device: &str, vendor: &str) -> Option<&'static str> {
    let device = device.to_ascii_lowercase();
    match (arch, vendor) {
        ("aarch64" | "arm64", _) if device.contains("bm1688") => Profile::SophonBm1688.name(),
        ("aarch64" | "arm64", _)
            if device.contains("nvidia,tegra264")
                || device.contains("dgx spark")
                || device.contains("nvidia gb10") =>
        {
            Profile::ThorSpark.name()
        }
        ("aarch64" | "arm64", _) if device.contains("nvidia,tegra234") => {
            Profile::JetsonOrin.name()
        }
        ("aarch64" | "arm64", "nvidia") => Profile::ArmCuda.name(),
        ("x86_64" | "amd64", "nvidia") => Profile::AmdCuda.name(),
        ("aarch64" | "arm64", _) => Profile::ArmCpu.name(),
        ("x86_64" | "amd64", _) => Profile::AmdCpu.name(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::detect;

    #[test]
    fn board_specific_profiles_take_precedence() {
        assert_eq!(
            detect("aarch64", "nvidia,tegra264", "nvidia"),
            Some("thor-spark")
        );
        assert_eq!(
            detect("aarch64", "NVIDIA DGX Spark", "nvidia"),
            Some("thor-spark")
        );
        assert_eq!(
            detect("aarch64", "nvidia,tegra234", "nvidia"),
            Some("jetson-orin")
        );
        assert_eq!(
            detect("aarch64", "sophgo,bm1688", ""),
            Some("sophon-bm1688")
        );
        assert_eq!(detect("aarch64", "", "nvidia"), Some("arm-cuda"));
        assert_eq!(detect("x86_64", "", "nvidia"), Some("amd-cuda"));
        assert_eq!(detect("aarch64", "", ""), Some("arm-cpu"));
        assert_eq!(detect("x86_64", "", "amd"), Some("amd-cpu"));
        assert_eq!(detect("riscv64", "", ""), None);
    }
}
