//! Net-class auto-detection from net names (partial port of
//! `kicad_tools.router.net_class`: `classify_from_name` and the pour-net
//! test `classify_and_apply_rules(...)[n].is_pour_net` reduces to).

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetClass {
    Power,
    Ground,
    HighCurrentSignal,
    Clock,
    HighSpeed,
    Differential,
    Analog,
    Rf,
    Debug,
    Signal,
}

impl NetClass {
    pub fn value(self) -> &'static str {
        match self {
            NetClass::Power => "power",
            NetClass::Ground => "ground",
            NetClass::HighCurrentSignal => "high_current_signal",
            NetClass::Clock => "clock",
            NetClass::HighSpeed => "high_speed",
            NetClass::Differential => "differential",
            NetClass::Analog => "analog",
            NetClass::Rf => "rf",
            NetClass::Debug => "debug",
            NetClass::Signal => "signal",
        }
    }
}

fn re(p: &str) -> Regex {
    RegexBuilder::new(p).case_insensitive(true).build().expect("net class regex")
}

/// `(?!_?LED)` is not supported by the `regex` crate; the first POWER
/// pattern is matched by [`power_prefix`] instead.
const POWER_PLACEHOLDER: &str = "<power-prefix>";

static PATTERNS: LazyLock<Vec<(NetClass, Vec<Option<Regex>>)>> = LazyLock::new(|| {
    let table: &[(NetClass, &[&str])] = &[
        (
            NetClass::Ground,
            &[
                r"^(GND|VSS|GNDA|GNDD|AGND|DGND|PGND|SGND|GROUND|CGND)$",
                r"^(CHASSIS|EARTH|SHIELD)$",
                r"_GND$|_VSS$|_AGND$|_DGND$",
            ],
        ),
        (
            NetClass::HighCurrentSignal,
            &[
                r"^PHASE_?[A-Z0-9]+$",
                r"^MOTOR_?[A-Z0-9]+$",
                r"^COIL_?[A-Z0-9]+$",
                r"^STATOR_?[A-Z0-9]+$",
                r"^ROTOR_?[A-Z0-9]+$",
                r"^SOLENOID_?[A-Z0-9]*$",
                r"^RELAY_?[A-Z0-9]*$",
            ],
        ),
        (
            NetClass::HighSpeed,
            &[
                r"(USB|ETH|HDMI|LVDS|PCIE|SDIO|QSPI|OSPI)",
                r"(MIPI|DSI|CSI|RGMII|RMII|MII)",
                r"(SATA|SAS|DP|DISPLAYPORT)",
                r"[_-](DP|DM|D[+-])$",
                r"(SD_D|SDIO_D|MMC_D)\d",
                r"(QSPI_D|OSPI_D)\d",
            ],
        ),
        (
            NetClass::Rf,
            &[
                r"(RF_|ANT_|ANTENNA)",
                r"(LNA|PA)_",
                r"^(RF|ANT|ANTENNA)\d*$",
                r"(TX_RF|RX_RF)",
            ],
        ),
        (
            NetClass::Debug,
            &[
                r"(SWDIO|SWCLK|SWDCLK|SWO)",
                r"(NRST|RESET|RST)",
                r"(TDI|TDO|TMS|TCK|TRST)",
                r"(DEBUG|DBG|TRACE)",
                r"(BOOT|PROG)",
            ],
        ),
        (
            NetClass::Clock,
            &[
                r"(CLK|CLOCK|MCLK|SCLK|PCLK|BCLK|LRCLK|FCLK|SYSCLK)",
                r"(OSC|XTAL|CRYSTAL|XIN|XOUT)",
                r"_CLK$|_SCK$",
                r"^(TCK|TCLK|JTCK)$",
            ],
        ),
        (
            NetClass::Power,
            &[
                POWER_PLACEHOLDER,
                r"^[+-]?\d+\.?\d*V[ADPS]?$",
                r"^[+-]?\d+V\d+[ADPS]?$",
                r"^(PVDD|PVCC|VBAT|VCORE|VCAP|VIO)$",
                r"_VCC$|_VDD$|_PWR$",
                r"^V\d+",
                r"^(VMOTOR|VMOT|VMAIN|VPWR|VDRIVE|VACT|VSRV)$",
            ],
        ),
        (
            NetClass::Analog,
            &[
                r"(AIN|AOUT|SENSE|FB|COMP|ISET)",
                r"^VREF",
                r"^AN\d+",
                r"(ADC|DAC)_?(CH)?\d*$",
                r"(AUDIO|MIC|SPK|LINE)(_[LRIO])?",
                r"(I2S|TDM|PDM)_(DIN|DOUT|SD|WS)",
            ],
        ),
        (
            NetClass::Differential,
            &[
                r"[_-]?[PN]$",
                r"[+-]$",
                r"_DIFF[PN]?$",
                r"(TX|RX)[PN]$",
                r"(CLK|DATA)[PN]$",
            ],
        ),
    ];
    table
        .iter()
        .map(|(c, ps)| {
            (
                *c,
                ps.iter()
                    .map(|p| (*p != POWER_PLACEHOLDER).then(|| re(p)))
                    .collect(),
            )
        })
        .collect()
});

/// `^(VCC|VDD|VBUS|VIN|VOUT|PWR(?!_?LED)|POWER(?!_?LED)|AVDD|DVDD)`.
fn power_prefix(name: &str) -> bool {
    let u = name.to_uppercase();
    for p in ["VCC", "VDD", "VBUS", "VIN", "VOUT", "AVDD", "DVDD"] {
        if u.starts_with(p) {
            return true;
        }
    }
    for p in ["PWR", "POWER"] {
        if let Some(rest) = u.strip_prefix(p) {
            if !(rest.starts_with("LED") || rest.starts_with("_LED")) {
                return true;
            }
        }
    }
    false
}

static SINGLE_ENDED_REFUSAL: LazyLock<Regex> = LazyLock::new(|| {
    // `diffpair._SINGLE_ENDED_REFUSAL_PATTERN`.
    re(r"^(.+_)?(CC|SBU)\d+$")
});

/// `diffpair.is_single_ended_refused`.
pub fn is_single_ended_refused(net_name: &str) -> bool {
    SINGLE_ENDED_REFUSAL.is_match(net_name)
}

/// `classify_from_name`.
pub fn classify_from_name(net_name: &str) -> Option<NetClass> {
    let upper = net_name.to_uppercase();
    for (class, patterns) in PATTERNS.iter() {
        for p in patterns {
            let hit = match p {
                Some(r) => r.is_match(&upper),
                None => power_prefix(&upper),
            };
            if hit {
                if *class == NetClass::Differential && is_single_ended_refused(net_name) {
                    continue;
                }
                return Some(*class);
            }
        }
    }
    None
}

/// Whether the auto-classifier marks `net_name` as a pour net
/// (`NET_CLASS_POWER` / Ground routing both carry `is_pour_net=True`).
pub fn is_pour_net(net_name: &str) -> bool {
    matches!(
        classify_from_name(net_name),
        Some(NetClass::Power | NetClass::Ground)
    )
}
