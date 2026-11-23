use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Auto,
    #[serde(alias = "thor", alias = "thor-spark")]
    JetsonThor,
    DgxSpark,
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
            Self::JetsonThor => Some("jetson-thor"),
            Self::DgxSpark => Some("dgx-spark"),
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
        ("aarch64" | "arm64", _) if device.contains("nvidia,tegra264") => {
            Profile::JetsonThor.name()
        }
        ("aarch64" | "arm64", _) if is_spark(&device) => Profile::DgxSpark.name(),
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

fn is_spark(device: &str) -> bool {
    device.contains("dgx spark") || device.contains("nvidia gb10")
}

pub fn saved(value: Option<String>, gpu_model: &str) -> Option<String> {
    value.map(|value| match value.as_str() {
        "thor-spark" | "thor" if is_spark(&gpu_model.to_ascii_lowercase()) => "dgx-spark".into(),
        "thor-spark" | "thor" => "jetson-thor".into(),
        "l4t" => "jetson-orin".into(),
        _ => value,
    })
}

pub fn compatible(profile: Option<&str>) -> Vec<String> {
    match profile {
        Some("dgx-spark") => vec!["dgx-spark".into(), "jetson-thor".into()],
        Some(profile) => vec![profile.into()],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Profile, compatible, detect, saved};

    #[test]
    fn board_specific_profiles_take_precedence() {
        assert_eq!(
            detect("aarch64", "nvidia,tegra264", "nvidia"),
            Some("jetson-thor")
        );
        assert_eq!(
            detect("aarch64", "NVIDIA DGX Spark", "nvidia"),
            Some("dgx-spark")
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

    #[test]
    fn spark_can_use_the_thor_profile_but_thor_cannot_use_the_spark_profile() {
        assert_eq!(detect("arm64", "NVIDIA GB10", "nvidia"), Some("dgx-spark"));
        assert_eq!(compatible(Some("dgx-spark")), ["dgx-spark", "jetson-thor"]);
        assert_eq!(compatible(Some("jetson-thor")), ["jetson-thor"]);
        assert_eq!(
            saved(Some("thor-spark".into()), "NVIDIA GB10").as_deref(),
            Some("dgx-spark")
        );
        assert_eq!(
            saved(Some("thor-spark".into()), "NVIDIA Thor").as_deref(),
            Some("jetson-thor")
        );
        assert_eq!(
            serde_json::from_str::<Profile>(r#""thor-spark""#)
                .unwrap()
                .name(),
            Some("jetson-thor")
        );
    }
}
