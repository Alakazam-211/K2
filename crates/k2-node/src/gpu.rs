//! GPUs as sysfs shows them (GPU jobs, 0.46.11, prd-compute-gpu-jobs-v1
//! §3.4). Files only, never a shell, world-readable, so the node (its
//! facts) and the root executor (which device a job gets) read the same
//! thing. Every reader takes the sysfs root, so tests run on a fixture
//! tree (`<root>/class/drm/renderD128/device -> …/0000:c4:00.0`).
//!
//! A GPU here is a PCI device with a **render node** (`renderD<N>`): the
//! only node v1 ever gives a job. Card (KMS) nodes are listed only so the
//! executor can hide them; ASPEED BMC graphics and anything else without
//! a known vendor id are not GPUs (`k2_node_proto::host::gpu_vendor`).

use std::path::{Path, PathBuf};

use k2_node_proto::gpu::GpuFact;

/// The live sysfs.
pub const SYS: &str = "/sys";
/// Where distros keep the PCI id names (`model`).
pub const PCI_IDS: &[&str] = &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"];

/// One GPU with a render node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysGpu {
    /// `0000:c4:00.0`.
    pub pci: String,
    /// `amd` | `intel` | `nvidia`.
    pub vendor: &'static str,
    /// `1002` / `1586` (lower-case hex, no `0x`).
    pub vendor_id: String,
    pub device_id: String,
    /// `amdgpu`, `i915`, `xe`, `nvidia`, or empty.
    pub driver: String,
    /// `renderD128`.
    pub render: String,
    /// Its card nodes (`card1`), hidden from GPU jobs.
    pub cards: Vec<String>,
    pub vram_bytes: Option<u64>,
    pub vis_vram_bytes: Option<u64>,
    pub gtt_bytes: Option<u64>,
}

impl SysGpu {
    /// `/dev/dri/renderD128`.
    pub fn render_path(&self) -> PathBuf {
        Path::new("/dev/dri").join(&self.render)
    }

    /// An APU or iGPU (system memory is its memory). sysfs has no flag for
    /// it, so: amdgpu whose CPU-visible VRAM is all of its VRAM (a small
    /// UMA carve-out), or Intel with no VRAM of its own.
    pub fn unified(&self) -> bool {
        match self.vendor {
            "amd" => matches!((self.vram_bytes, self.vis_vram_bytes), (Some(v), Some(vis)) if v > 0 && v == vis),
            "intel" => self.vram_bytes.is_none_or(|v| v == 0),
            _ => false,
        }
    }

    /// Its facts (`jobs` false: the node sets it per session).
    pub fn fact(&self, model: Option<String>) -> GpuFact {
        let ids = format!("{}:{}", self.vendor_id, self.device_id);
        GpuFact {
            vendor: self.vendor.to_string(),
            pci: self.pci.clone(),
            driver: self.driver.clone(),
            model: model.unwrap_or_else(|| ids.clone()),
            ids,
            render: self.render.clone(),
            vram_bytes: self.vram_bytes,
            gtt_bytes: self.gtt_bytes,
            unified: self.unified(),
            jobs: false,
        }
    }
}

/// A PCI address as sysfs writes it: `dddd:bb:dd.f`, lower-case hex.
pub fn valid_pci(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 12
        && b[4] == b':'
        && b[7] == b':'
        && b[10] == b'.'
        && b.iter().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10) || matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn read_u64(p: &Path) -> Option<u64> {
    read_trim(p)?.parse().ok()
}

/// The last component of a link (`device -> ../../../0000:c4:00.0`).
fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok()?.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn hex_id(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches("0x").to_ascii_lowercase();
    (h.len() == 4 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// Every GPU with a render node under `sys` (`/sys`), by render node name.
pub fn discover(sys: &Path) -> Vec<SysGpu> {
    let drm = sys.join("class/drm");
    let Ok(rd) = std::fs::read_dir(&drm) else { return Vec::new() };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut out: Vec<SysGpu> = Vec::new();
    for n in names.iter().filter(|n| n.strip_prefix("renderD").is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))) {
        let dev = drm.join(n).join("device");
        let Some(pci) = link_name(&dev).filter(|p| valid_pci(p)) else { continue };
        let Some(vendor_id) = read_trim(&dev.join("vendor")).and_then(|v| hex_id(&v)) else { continue };
        let Some(vendor) = k2_node_proto::host::gpu_vendor(&format!("0x{vendor_id}")) else { continue };
        let device_id = read_trim(&dev.join("device")).and_then(|v| hex_id(&v)).unwrap_or_default();
        let cards = names
            .iter()
            .filter(|c| c.strip_prefix("card").is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())))
            .filter(|c| link_name(&drm.join(c).join("device")).as_deref() == Some(pci.as_str()))
            .cloned()
            .collect();
        out.push(SysGpu {
            pci,
            vendor,
            vendor_id,
            device_id,
            driver: link_name(&dev.join("driver")).unwrap_or_default(),
            render: n.clone(),
            cards,
            vram_bytes: read_u64(&dev.join("mem_info_vram_total")),
            vis_vram_bytes: read_u64(&dev.join("mem_info_vis_vram_total")),
            gtt_bytes: read_u64(&dev.join("mem_info_gtt_total")),
        });
    }
    out
}

/// Every DRM node under `sys` (`card*`, `renderD*`), for the executor to
/// hide the ones a job doesn't get.
pub fn drm_nodes(sys: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(sys.join("class/drm")) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| (n.starts_with("card") || n.starts_with("renderD")) && !n.contains('-'))
        .collect();
    v.sort();
    v
}

/// The model name from a `pci.ids` text: `AMD Test APU [Radeon Graphics]`.
pub fn pci_model(pci_ids: &str, vendor_id: &str, device_id: &str) -> Option<String> {
    let mut in_vendor = false;
    for line in pci_ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line.get(..4).is_some_and(|v| v.eq_ignore_ascii_case(vendor_id));
            continue;
        }
        if in_vendor && !line.starts_with("\t\t") {
            let l = &line[1..];
            if l.get(..4).is_some_and(|d| d.eq_ignore_ascii_case(device_id)) {
                let name: String = l[4..].trim().chars().filter(|c| !c.is_control()).take(96).collect();
                let brand = match vendor_id {
                    "1002" => "AMD",
                    "8086" => "Intel",
                    "10de" => "NVIDIA",
                    _ => "",
                };
                return (!name.is_empty()).then(|| format!("{brand} {name}").trim().to_string());
            }
        }
    }
    None
}

/// The facts of every GPU on this machine (`HostFacts.gpus`).
pub fn facts(sys: &Path) -> Vec<GpuFact> {
    let gpus = discover(sys);
    if gpus.is_empty() {
        return Vec::new();
    }
    let ids = PCI_IDS.iter().find_map(|p| std::fs::read_to_string(p).ok());
    gpus.iter().map(|g| g.fact(ids.as_deref().and_then(|t| pci_model(t, &g.vendor_id, &g.device_id)))).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sysfs tree: `(render, card, pci, vendor, device, driver, vram,
    /// vis_vram, gtt)` per GPU, plus loose card nodes (ASPEED).
    pub(crate) fn fixture(root: &Path, gpus: &[(&str, &str, &str, &str, &str, &str, Option<u64>, Option<u64>, Option<u64>)]) {
        let drm = root.join("class/drm");
        std::fs::create_dir_all(&drm).unwrap();
        for (render, card, pci, vendor, device, driver, vram, vis, gtt) in gpus {
            let dev = root.join("devices/pci0000:00").join(pci);
            std::fs::create_dir_all(&dev).unwrap();
            std::fs::write(dev.join("vendor"), format!("{vendor}\n")).unwrap();
            std::fs::write(dev.join("device"), format!("{device}\n")).unwrap();
            let drv = root.join("bus/pci/drivers").join(driver);
            std::fs::create_dir_all(&drv).unwrap();
            std::os::unix::fs::symlink(&drv, dev.join("driver")).unwrap();
            for (k, v) in [("mem_info_vram_total", vram), ("mem_info_vis_vram_total", vis), ("mem_info_gtt_total", gtt)] {
                if let Some(v) = v {
                    std::fs::write(dev.join(k), format!("{v}\n")).unwrap();
                }
            }
            for node in [render, card] {
                if node.is_empty() {
                    continue;
                }
                std::fs::create_dir_all(drm.join(node)).unwrap();
                std::os::unix::fs::symlink(&dev, drm.join(node).join("device")).unwrap();
            }
        }
    }

    const STRIX: (&str, &str, &str, &str, &str, &str, Option<u64>, Option<u64>, Option<u64>) =
        ("renderD128", "card1", "0000:c4:00.0", "0x1002", "0x1586", "amdgpu", Some(512 << 20), Some(512 << 20), Some(96 << 30));

    #[test]
    fn an_amdgpu_apu_tree_is_one_unified_gpu() {
        let d = crate::util::temp_dir("gpu-sysfs");
        fixture(&d, &[STRIX]);
        let g = discover(&d);
        assert_eq!(g.len(), 1, "{g:?}");
        let g = &g[0];
        assert_eq!((g.pci.as_str(), g.vendor, g.render.as_str(), g.driver.as_str()), ("0000:c4:00.0", "amd", "renderD128", "amdgpu"));
        assert_eq!(g.cards, vec!["card1".to_string()]);
        assert!(g.unified(), "VRAM all CPU-visible: an APU");
        assert_eq!(g.render_path(), Path::new("/dev/dri/renderD128"));
        let f = g.fact(Some("AMD Radeon Test APU".into()));
        assert_eq!((f.ids.as_str(), f.unified, f.jobs), ("1002:1586", true, false));
        assert_eq!(f.line(), "AMD Radeon Test APU (amdgpu, 96G unified)");
    }

    #[test]
    fn an_aspeed_only_tree_has_no_gpu() {
        let d = crate::util::temp_dir("gpu-aspeed");
        fixture(&d, &[("", "card0", "0000:03:00.0", "0x1a03", "0x2000", "ast", None, None, None)]);
        assert!(discover(&d).is_empty());
        assert!(facts(&d).is_empty());
        assert_eq!(drm_nodes(&d), vec!["card0".to_string()]);
    }

    #[test]
    fn a_discrete_amd_card_is_not_unified() {
        let d = crate::util::temp_dir("gpu-dgpu");
        fixture(&d, &[("renderD129", "card2", "0000:03:00.0", "0x1002", "0x744c", "amdgpu", Some(24 << 30), Some(256 << 20), Some(32 << 30))]);
        let g = discover(&d);
        assert!(!g[0].unified());
    }

    #[test]
    fn pci_ids_give_the_model() {
        let ids = "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t1586  Test APU [Radeon Graphics]\n\t\t1002 0001  Sub\n1a03  ASPEED\n\t2000  ASPEED Graphics\n";
        assert_eq!(pci_model(ids, "1002", "1586").as_deref(), Some("AMD Test APU [Radeon Graphics]"));
        assert_eq!(pci_model(ids, "1002", "ffff"), None);
        assert_eq!(pci_model(ids, "8086", "a780"), None);
    }

    #[test]
    fn pci_addresses_are_strict() {
        assert!(valid_pci("0000:c4:00.0"));
        for bad in ["0000:C4:00.0", "c4:00.0", "0000:c4:00.0/..", "../../x", ""] {
            assert!(!valid_pci(bad), "{bad}");
        }
    }
}
