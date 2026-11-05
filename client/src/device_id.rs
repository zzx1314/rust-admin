use crate::error::ClientError;

/// Sources a device id can be derived from, in priority order.
pub const SOURCE_DEVICE_TREE_SERIAL: &str = "device-tree-serial";
pub const SOURCE_CPU_SERIAL: &str = "cpu-serial";
pub const SOURCE_EMMC_CID: &str = "emmc-cid";
pub const SOURCE_DMI_PRODUCT_UUID: &str = "dmi-product-uuid";
pub const SOURCE_DMI_PRODUCT_SERIAL: &str = "dmi-product-serial";
pub const SOURCE_DMI_BOARD_SERIAL: &str = "dmi-board-serial";
pub const SOURCE_MAC_ADDRESS: &str = "mac-address";
pub const SOURCE_MACHINE_ID: &str = "machine-id";
/// Id supplied by the user (`--device-id` / `DOG_DEVICE_ID`).
pub const SOURCE_EXPLICIT: &str = "explicit";
/// Id read back from the pinned state file (`--device-id-file`).
pub const SOURCE_PINNED: &str = "pinned";

/// A device id together with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub id: String,
    pub source: &'static str,
}

impl DeviceIdentity {
    /// Use a caller supplied id as-is (only trimmed).
    pub fn explicit(raw: &str) -> Result<Self, ClientError> {
        let id = raw.trim();
        if id.is_empty() {
            return Err(ClientError::DeviceId(
                "device id must not be empty".to_string(),
            ));
        }
        if id.len() > 128 {
            return Err(ClientError::DeviceId(
                "device id must not exceed 128 characters".to_string(),
            ));
        }
        Ok(Self {
            id: id.to_string(),
            source: SOURCE_EXPLICIT,
        })
    }
}

/// Outcome of reading a hardware information file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileProbe {
    Value(String),
    Missing,
    /// The file exists but the client has no permission (typical for
    /// `/sys/class/dmi/id/*` and `/sys/block/mmcblk*/device/cid` on Ubuntu,
    /// which are mode `0400` root-only).
    PermissionDenied,
    Error(String),
}

/// Abstract access to hardware information so detection is testable.
pub trait HardwareProbe {
    fn read_file(&self, path: &str) -> FileProbe;
    fn list_dir(&self, path: &str) -> Vec<String>;
    fn exists(&self, path: &str) -> bool {
        matches!(self.read_file(path), FileProbe::Value(_)) || !self.list_dir(path).is_empty()
    }
}

/// Reads hardware information from the real file system (`sysfs`/`procfs`).
pub struct SystemProbe;

impl HardwareProbe for SystemProbe {
    fn read_file(&self, path: &str) -> FileProbe {
        match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                if text.is_empty() {
                    FileProbe::Missing
                } else {
                    FileProbe::Value(text)
                }
            }
            Err(error) => match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                    FileProbe::Missing
                }
                std::io::ErrorKind::PermissionDenied => FileProbe::PermissionDenied,
                other => FileProbe::Error(format!("{other}: {error}")),
            },
        }
    }

    fn list_dir(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
}

/// Status of one candidate hardware identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateStatus {
    /// Usable identifier (already normalised).
    Found(String),
    /// Read successfully but unusable (placeholder, wrong length, virtual MAC, ...).
    Rejected(String),
    /// Exists but cannot be read by the current user.
    PermissionDenied,
    /// Not present on this machine.
    Missing,
    /// Read failed for another reason.
    Error(String),
}

impl CandidateStatus {
    fn from_file(
        probe: &impl HardwareProbe,
        path: &str,
        normalise: impl Fn(&str) -> Option<String>,
        reject_reason: &str,
    ) -> Self {
        match probe.read_file(path) {
            FileProbe::Value(raw) => match normalise(&raw) {
                Some(id) => Self::Found(id),
                None => Self::Rejected(reject_reason.to_string()),
            },
            FileProbe::Missing => Self::Missing,
            FileProbe::PermissionDenied => Self::PermissionDenied,
            FileProbe::Error(error) => Self::Error(error),
        }
    }
}

/// One probed location and what came out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub source: &'static str,
    pub location: String,
    pub status: CandidateStatus,
}

/// Full report of the device id inspection (`--explain-device-id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdReport {
    pub identity: Option<DeviceIdentity>,
    pub candidates: Vec<Candidate>,
}

impl DeviceIdReport {
    /// Sources skipped only because of missing permissions.
    pub fn permission_denied_sources(&self) -> Vec<&'static str> {
        let mut sources: Vec<&'static str> = self
            .candidates
            .iter()
            .filter(|candidate| candidate.status == CandidateStatus::PermissionDenied)
            .map(|candidate| candidate.source)
            .collect();
        sources.dedup();
        sources
    }

    pub fn has_permission_denied(&self) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.status == CandidateStatus::PermissionDenied)
    }
}

/// Resolve the device id: explicit value first, hardware detection otherwise.
///
/// An explicitly supplied (but blank) id is an error rather than a silent
/// fallback, so configuration mistakes are visible.
pub fn resolve(explicit: Option<&str>) -> Result<DeviceIdentity, ClientError> {
    match explicit {
        Some(value) => DeviceIdentity::explicit(value),
        None => detect(),
    }
}

/// Detect a device id from hardware information.
pub fn detect() -> Result<DeviceIdentity, ClientError> {
    detect_with(&SystemProbe)
}

/// Resolve the device id with an optional pin file.
///
/// Order:
/// 1. explicit `--device-id` / `DOG_DEVICE_ID`;
/// 2. the id pinned in `file` (if the file exists and is not empty);
/// 3. hardware detection — the result is then written to `file` so later runs
///    (possibly started by a different user, e.g. a systemd service) keep the
///    same id even when `sysfs` permissions differ.
pub fn resolve_persisted(
    explicit: Option<&str>,
    file: Option<&std::path::Path>,
) -> Result<DeviceIdentity, ClientError> {
    if let Some(value) = explicit {
        return DeviceIdentity::explicit(value);
    }

    let Some(path) = file else {
        return detect();
    };

    if let Ok(contents) = std::fs::read_to_string(path) {
        let stored = contents.lines().next().unwrap_or("").trim();
        if !stored.is_empty() {
            let mut identity = DeviceIdentity::explicit(stored)?;
            identity.source = SOURCE_PINNED;
            tracing::debug!(path = %path.display(), "device id loaded from pin file");
            return Ok(identity);
        }
    }

    let identity = detect()?;
    persist_device_id(path, &identity);
    Ok(identity)
}

fn persist_device_id(path: &std::path::Path, identity: &DeviceIdentity) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(path = %parent.display(), %error, "cannot create device id directory");
        return;
    }

    match std::fs::write(path, format!("{}\n", identity.id)) {
        Ok(()) => tracing::info!(
            path = %path.display(),
            device_id = %identity.id,
            source = identity.source,
            "device id pinned to file"
        ),
        Err(error) => tracing::warn!(
            path = %path.display(),
            %error,
            "cannot write device id pin file, the id will be re-detected on next start"
        ),
    }
}

/// Inspect every hardware source and explain which one is used and why the
/// others are not.
pub fn explain() -> DeviceIdReport {
    inspect_with(&SystemProbe)
}

/// Detect a device id using the given probe.
///
/// Works on Ubuntu (both x86 and ARM): device tree serial and eMMC CID are used
/// when present/readable, DMI identifiers on x86, then the physical NIC MAC and
/// finally `/etc/machine-id`.
pub fn detect_with(probe: &impl HardwareProbe) -> Result<DeviceIdentity, ClientError> {
    let report = inspect_with(probe);

    let Some(identity) = report.identity.clone() else {
        return Err(ClientError::DeviceId(
            "no hardware identifier found (serial, eMMC CID, DMI UUID, MAC, machine-id); \
             pass --device-id / DOG_DEVICE_ID instead"
                .to_string(),
        ));
    };

    if identity.source == SOURCE_MACHINE_ID {
        tracing::warn!(
            "device id derived from the OS machine-id; cloned system images share this value, \
             prefer a board serial / eMMC CID or pass --device-id"
        );
    } else if report.has_permission_denied() && identity.source == SOURCE_MAC_ADDRESS {
        tracing::warn!(
            denied = ?report.permission_denied_sources(),
            "some hardware identifiers are only readable by root (typical on Ubuntu); \
             run as root or as a systemd service to use a more stable id"
        );
    }

    Ok(identity)
}

/// Build the full detection report without logging.
pub fn inspect_with(probe: &impl HardwareProbe) -> DeviceIdReport {
    let candidates = collect_candidates(probe);
    let identity = candidates
        .iter()
        .find_map(|candidate| match &candidate.status {
            CandidateStatus::Found(id) => Some(DeviceIdentity {
                id: id.clone(),
                source: candidate.source,
            }),
            _ => None,
        });

    DeviceIdReport {
        identity,
        candidates,
    }
}

fn collect_candidates(probe: &impl HardwareProbe) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    // 1. board serial from the device tree (RK35xx and other ARM boards)
    for path in [
        "/sys/firmware/devicetree/base/serial-number",
        "/proc/device-tree/serial-number",
    ] {
        candidates.push(Candidate {
            source: SOURCE_DEVICE_TREE_SERIAL,
            location: path.to_string(),
            status: CandidateStatus::from_file(
                probe,
                path,
                normalize_serial,
                "invalid board serial number",
            ),
        });
    }

    // 2. CPU serial (/proc/cpuinfo "Serial", Raspberry Pi and similar)
    candidates.push(Candidate {
        source: SOURCE_CPU_SERIAL,
        location: "/proc/cpuinfo (Serial)".to_string(),
        status: cpu_serial_status(probe),
    });

    // 3. eMMC CID (hardware unique, but mode 0400 on Ubuntu)
    let mut block_devices = probe.list_dir("/sys/block");
    block_devices.sort();
    for device in block_devices
        .iter()
        .filter(|name| name.starts_with("mmcblk"))
    {
        let path = format!("/sys/block/{device}/device/cid");
        candidates.push(Candidate {
            source: SOURCE_EMMC_CID,
            location: path.clone(),
            status: CandidateStatus::from_file(probe, &path, normalize_serial, "invalid eMMC CID"),
        });
    }

    // 4. DMI identifiers (x86 / UEFI, mode 0400 on Ubuntu)
    for (path, source) in [
        ("/sys/class/dmi/id/product_uuid", SOURCE_DMI_PRODUCT_UUID),
        (
            "/sys/class/dmi/id/product_serial",
            SOURCE_DMI_PRODUCT_SERIAL,
        ),
        ("/sys/class/dmi/id/board_serial", SOURCE_DMI_BOARD_SERIAL),
    ] {
        candidates.push(Candidate {
            source,
            location: path.to_string(),
            status: CandidateStatus::from_file(
                probe,
                path,
                normalize_serial,
                "placeholder DMI identifier",
            ),
        });
    }

    // 5. physical NIC MAC
    candidates.extend(mac_candidates(probe));

    // 6. OS machine-id (last resort: shared by cloned images)
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        candidates.push(Candidate {
            source: SOURCE_MACHINE_ID,
            location: path.to_string(),
            status: CandidateStatus::from_file(
                probe,
                path,
                normalize_machine_id,
                "invalid machine-id",
            ),
        });
    }

    candidates
}

fn cpu_serial_status(probe: &impl HardwareProbe) -> CandidateStatus {
    match probe.read_file("/proc/cpuinfo") {
        FileProbe::Value(contents) => {
            let serial = contents.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("serial")
                    .then(|| value.trim())
            });

            match serial {
                Some(value) => match normalize_serial(value) {
                    Some(id) => CandidateStatus::Found(id),
                    None => CandidateStatus::Rejected("invalid CPU serial".to_string()),
                },
                // x86 kernels do not expose a CPU serial at all.
                None => CandidateStatus::Missing,
            }
        }
        FileProbe::Missing => CandidateStatus::Missing,
        FileProbe::PermissionDenied => CandidateStatus::PermissionDenied,
        FileProbe::Error(error) => CandidateStatus::Error(error),
    }
}

fn mac_candidates(probe: &impl HardwareProbe) -> Vec<Candidate> {
    let mut interfaces = probe.list_dir("/sys/class/net");
    interfaces.sort_by_key(|name| mac_priority(name));

    interfaces
        .into_iter()
        .map(|interface| {
            let path = format!("/sys/class/net/{interface}/address");
            let status = if !is_physical_interface(probe, &interface) {
                CandidateStatus::Rejected("virtual interface".to_string())
            } else {
                match probe.read_file(&path) {
                    FileProbe::Value(raw) => match normalize_mac(&raw) {
                        None => CandidateStatus::Rejected("invalid MAC address".to_string()),
                        Some(mac) if !is_globally_administered(&mac) => CandidateStatus::Rejected(
                            "locally administered (random/virtual) MAC".to_string(),
                        ),
                        Some(mac) => CandidateStatus::Found(mac),
                    },
                    FileProbe::Missing => CandidateStatus::Missing,
                    FileProbe::PermissionDenied => CandidateStatus::PermissionDenied,
                    FileProbe::Error(error) => CandidateStatus::Error(error),
                }
            };

            Candidate {
                source: SOURCE_MAC_ADDRESS,
                location: path,
                status,
            }
        })
        .collect()
}

/// Normalise a serial-like value to upper-case ASCII alphanumerics.
///
/// Values that are too short/long or obvious placeholders (`NONE`, all zeros,
/// all `F`) are rejected because they would collide across devices.
pub fn normalize_serial(raw: &str) -> Option<String> {
    let cleaned = normalize_hex_text(raw);
    if cleaned.len() < 6 {
        return None;
    }
    Some(cleaned)
}

fn normalize_machine_id(raw: &str) -> Option<String> {
    let cleaned = normalize_hex_text(raw);
    // systemd machine-ids are 32 hex characters
    if cleaned.len() < 32 {
        return None;
    }
    Some(cleaned)
}

fn normalize_hex_text(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_uppercase();

    if cleaned.len() > 64 {
        return cleaned[..64].to_string();
    }

    const PLACEHOLDERS: &[&str] = &[
        "NONE",
        "UNKNOWN",
        "DEFAULT",
        "TOBEFILLEDBYOEM",
        "SERIALNUMBER",
        "NOTSPECIFIED",
        "0123456789ABCDEF",
        "1234567890",
        "12345678",
    ];

    if PLACEHOLDERS.contains(&cleaned.as_str())
        || (!cleaned.is_empty() && cleaned.chars().all(|character| character == '0'))
        || (!cleaned.is_empty() && cleaned.chars().all(|character| character == 'F'))
    {
        String::new()
    } else {
        cleaned
    }
}

fn mac_priority(name: &str) -> u8 {
    if name == "end0" || name == "end1" {
        0
    } else if name.starts_with("eth") {
        1
    } else if name.starts_with("en") {
        2
    } else if name.starts_with("wl") || name.starts_with("wlan") {
        3
    } else {
        4
    }
}

fn is_physical_interface(probe: &impl HardwareProbe, name: &str) -> bool {
    if name == "lo" {
        return false;
    }

    const VIRTUAL_PREFIXES: &[&str] = &[
        "docker",
        "veth",
        "br-",
        "virbr",
        "tun",
        "tap",
        "wg",
        "zt",
        "bond",
        "team",
        "dummy",
        "ip6",
        "sit",
        "macvtap",
        "vxlan",
        "tailscale",
        "cni",
        "flannel",
        "kube",
        "cali",
    ];
    if VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }

    // Virtual interfaces have no `device` link in sysfs.
    probe.exists(&format!("/sys/class/net/{name}/device"))
}

fn normalize_mac(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_uppercase();

    if cleaned.len() != 12 || cleaned.chars().all(|character| character == '0') {
        return None;
    }

    Some(cleaned)
}

fn is_globally_administered(mac: &str) -> bool {
    u8::from_str_radix(&mac[..2], 16)
        .map(|first_octet| first_octet & 0x02 == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeProbe {
        files: HashMap<String, FileProbe>,
        dirs: HashMap<String, Vec<String>>,
    }

    impl FakeProbe {
        fn file(mut self, path: &str, content: &str) -> Self {
            self.files
                .insert(path.to_string(), FileProbe::Value(content.to_string()));
            self
        }

        fn denied(mut self, path: &str) -> Self {
            self.files
                .insert(path.to_string(), FileProbe::PermissionDenied);
            self
        }

        fn dir(mut self, path: &str, entries: &[&str]) -> Self {
            self.dirs.insert(
                path.to_string(),
                entries.iter().map(|entry| entry.to_string()).collect(),
            );
            self
        }
    }

    impl HardwareProbe for FakeProbe {
        fn read_file(&self, path: &str) -> FileProbe {
            self.files.get(path).cloned().unwrap_or(FileProbe::Missing)
        }

        fn list_dir(&self, path: &str) -> Vec<String> {
            self.dirs.get(path).cloned().unwrap_or_default()
        }

        fn exists(&self, path: &str) -> bool {
            self.files.contains_key(path) || self.dirs.contains_key(path)
        }
    }

    #[test]
    fn prefere_device_tree_serial() {
        let probe = FakeProbe::default()
            .file("/sys/firmware/devicetree/base/serial-number", "abc123def\0")
            .file(
                "/sys/class/dmi/id/product_uuid",
                "11111111-2222-3333-4444-555555555555",
            );
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.id, "ABC123DEF");
        assert_eq!(identity.source, SOURCE_DEVICE_TREE_SERIAL);
    }

    #[test]
    fn falls_back_to_cpu_serial() {
        let probe = FakeProbe::default().file(
            "/proc/cpuinfo",
            "processor\t: 0\nSerial\t\t: 00000000deadbeef\n",
        );
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.id, "00000000DEADBEEF");
        assert_eq!(identity.source, SOURCE_CPU_SERIAL);
    }

    #[test]
    fn x86_cpuinfo_without_serial_is_skipped() {
        let probe = FakeProbe::default()
            .file("/proc/cpuinfo", "processor\t: 0\nmodel name\t: Intel\n")
            .file("/sys/class/dmi/id/product_serial", "PF51D049");
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.source, SOURCE_DMI_PRODUCT_SERIAL);
        assert_eq!(identity.id, "PF51D049");
    }

    #[test]
    fn falls_back_to_emmc_cid() {
        let probe = FakeProbe::default()
            .dir("/sys/block", &["mmcblk0", "loop0"])
            .file(
                "/sys/block/mmcblk0/device/cid",
                "150100334d423247420196f3a2a1be00",
            );
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.source, SOURCE_EMMC_CID);
        assert!(identity.id.starts_with("150100334D423247"));
    }

    #[test]
    fn normalises_dmi_uuid() {
        let probe = FakeProbe::default().file(
            "/sys/class/dmi/id/product_uuid",
            "4c4c4544-0042-3510-8043-b4c04f4e4d32\n",
        );
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.id, "4C4C4544004235108043B4C04F4E4D32");
        assert_eq!(identity.source, SOURCE_DMI_PRODUCT_UUID);
    }

    #[test]
    fn rejects_placeholder_dmi_uuid() {
        let probe = FakeProbe::default()
            .file(
                "/sys/class/dmi/id/product_uuid",
                "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
            )
            .file("/etc/machine-id", "0123456789abcdef0123456789abcdef");
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.source, SOURCE_MACHINE_ID);
    }

    #[test]
    fn picks_physical_mac_and_skips_virtual_interfaces() {
        let probe = FakeProbe::default()
            .dir(
                "/sys/class/net",
                &["lo", "docker0", "veth1234", "wlan0", "eth0"],
            )
            .dir("/sys/class/net/eth0/device", &["uevent"])
            .dir("/sys/class/net/wlan0/device", &["uevent"])
            .file("/sys/class/net/eth0/address", "a4:bb:6d:11:22:33")
            .file("/sys/class/net/wlan0/address", "a4:bb:6d:44:55:66")
            .file("/sys/class/net/docker0/address", "02:42:ac:11:00:02")
            .file("/sys/class/net/lo/address", "00:00:00:00:00:00");

        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.id, "A4BB6D112233");
        assert_eq!(identity.source, SOURCE_MAC_ADDRESS);
    }

    #[test]
    fn skips_locally_administered_mac() {
        let probe = FakeProbe::default()
            .dir("/sys/class/net", &["eth0"])
            .dir("/sys/class/net/eth0/device", &["uevent"])
            .file("/sys/class/net/eth0/address", "02:42:ac:11:00:02")
            .file("/etc/machine-id", "aabbccddeeff00112233445566778899");
        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.source, SOURCE_MACHINE_ID);
    }

    #[test]
    fn ubuntu_root_only_dmi_falls_back_to_mac() {
        // On Ubuntu `/sys/class/dmi/id/*` is mode 0400, so a non-root client
        // cannot read the UUID and must still produce a stable id from the NIC.
        let probe = FakeProbe::default()
            .denied("/sys/class/dmi/id/product_uuid")
            .denied("/sys/class/dmi/id/product_serial")
            .denied("/sys/class/dmi/id/board_serial")
            .dir("/sys/block", &["mmcblk0"])
            .denied("/sys/block/mmcblk0/device/cid")
            .dir("/sys/class/net", &["enp0s31f6"])
            .dir("/sys/class/net/enp0s31f6/device", &["uevent"])
            .file("/sys/class/net/enp0s31f6/address", "c4:c6:e6:6e:99:8e")
            .file("/etc/machine-id", "c5157cdebdb24db996db95ffa0035bf2");

        let identity = detect_with(&probe).unwrap();
        assert_eq!(identity.id, "C4C6E66E998E");
        assert_eq!(identity.source, SOURCE_MAC_ADDRESS);

        let report = inspect_with(&probe);
        assert!(report.has_permission_denied());
        assert_eq!(
            report.permission_denied_sources(),
            vec![
                SOURCE_EMMC_CID,
                SOURCE_DMI_PRODUCT_UUID,
                SOURCE_DMI_PRODUCT_SERIAL,
                SOURCE_DMI_BOARD_SERIAL
            ]
        );
    }

    #[test]
    fn explain_reports_every_candidate_in_priority_order() {
        let probe = FakeProbe::default()
            .dir("/sys/class/net", &["lo", "enp0s31f6"])
            .dir("/sys/class/net/enp0s31f6/device", &["uevent"])
            .file("/sys/class/net/enp0s31f6/address", "c4:c6:e6:6e:99:8e")
            .file("/etc/machine-id", "c5157cdebdb24db996db95ffa0035bf2");

        let report = inspect_with(&probe);
        let sources: Vec<&str> = report
            .candidates
            .iter()
            .map(|candidate| candidate.source)
            .collect();

        assert_eq!(sources.first(), Some(&SOURCE_DEVICE_TREE_SERIAL));
        assert!(sources.contains(&SOURCE_CPU_SERIAL));
        assert!(sources.contains(&SOURCE_DMI_PRODUCT_UUID));
        assert!(sources.contains(&SOURCE_MAC_ADDRESS));
        assert_eq!(sources.last(), Some(&SOURCE_MACHINE_ID));
        assert_eq!(
            report.identity.as_ref().map(|identity| identity.source),
            Some(SOURCE_MAC_ADDRESS)
        );
    }

    #[test]
    fn errors_without_any_hardware_source() {
        let error = detect_with(&FakeProbe::default()).unwrap_err();
        assert!(matches!(error, ClientError::DeviceId(_)));
        assert!(inspect_with(&FakeProbe::default()).identity.is_none());
    }

    #[test]
    fn explicit_id_is_kept_verbatim() {
        let identity = DeviceIdentity::explicit("  dog-001 ").unwrap();
        assert_eq!(identity.id, "dog-001");
        assert_eq!(identity.source, SOURCE_EXPLICIT);
    }

    #[test]
    fn resolve_prefers_explicit_value() {
        let identity = resolve(Some("00022025C0600001")).unwrap();
        assert_eq!(identity.id, "00022025C0600001");
        assert_eq!(identity.source, SOURCE_EXPLICIT);

        // a blank explicit id is an error, not a silent hardware fallback
        assert!(matches!(
            resolve(Some("   ")),
            Err(ClientError::DeviceId(_))
        ));
    }

    #[test]
    fn pinned_file_wins_over_detection() {
        let dir = std::env::temp_dir().join(format!(
            "dog_device_id_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device-id");
        std::fs::write(&path, "PINNED-ID-0001\n").unwrap();

        let identity = resolve_persisted(None, Some(&path)).unwrap();
        assert_eq!(identity.id, "PINNED-ID-0001");
        assert_eq!(identity.source, SOURCE_PINNED);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detection_result_is_pinned_to_file() {
        let dir = std::env::temp_dir().join(format!(
            "dog_device_id_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device-id");

        // no explicit id -> hardware detection (works on real machines only,
        // so this assertion is guarded)
        match resolve_persisted(None, Some(&path)) {
            Ok(identity) => {
                let stored = std::fs::read_to_string(&path).unwrap();
                assert_eq!(stored.trim(), identity.id);
                let again = resolve_persisted(None, Some(&path)).unwrap();
                assert_eq!(again.id, identity.id);
                assert_eq!(again.source, SOURCE_PINNED);
            }
            Err(error) => {
                // machines without any hardware identifier (CI containers)
                assert!(matches!(error, ClientError::DeviceId(_)));
            }
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_id_ignores_pin_file() {
        let path = std::env::temp_dir().join("dog_device_id_never_used");
        let identity = resolve_persisted(Some("dog-001"), Some(&path)).unwrap();
        assert_eq!(identity.id, "dog-001");
        assert_eq!(identity.source, SOURCE_EXPLICIT);
        assert!(!path.exists());
    }

    #[test]
    fn normalize_serial_rejects_short_and_placeholder_values() {
        assert!(normalize_serial("abc").is_none());
        assert!(normalize_serial("NONE").is_none());
        assert!(normalize_serial("0000000000").is_none());
        assert!(normalize_serial("FFFFFFFF").is_none());
        assert_eq!(normalize_serial(" abc-123 ").unwrap(), "ABC123");
    }
}
