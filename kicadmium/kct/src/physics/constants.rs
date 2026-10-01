//! Physical constants and material properties (port of `physics.constants`).

/// Speed of light in vacuum, m/s.
pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;
/// Vacuum permittivity, F/m.
pub const VACUUM_PERMITTIVITY: f64 = 8.854187817e-12;
/// Vacuum permeability, H/m.
pub const VACUUM_PERMEABILITY: f64 = 1.2566370614e-6;
/// Copper conductivity at 20 C, S/m.
pub const COPPER_CONDUCTIVITY: f64 = 5.8e7;

/// Copper foil specification by weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CopperWeight {
    /// Weight in oz/ft^2.
    pub oz: f64,
    pub thickness_um: f64,
    pub thickness_mm: f64,
}

impl CopperWeight {
    /// 1 oz/ft^2 = 35 um (approximately).
    pub const fn from_oz(oz: f64) -> Self {
        let thickness_um = oz * 35.0;
        Self {
            oz,
            thickness_um,
            thickness_mm: thickness_um / 1000.0,
        }
    }
}

pub const COPPER_HALF_OZ: CopperWeight = CopperWeight::from_oz(0.5);
pub const COPPER_1OZ: CopperWeight = CopperWeight::from_oz(1.0);
pub const COPPER_2OZ: CopperWeight = CopperWeight::from_oz(2.0);

/// Dielectric material properties.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DielectricMaterial {
    pub name: &'static str,
    /// Relative permittivity.
    pub epsilon_r: f64,
    /// tan(delta) at 1 GHz.
    pub loss_tangent: f64,
    pub description: &'static str,
}

impl DielectricMaterial {
    /// Rough microstrip effective epsilon estimate.
    pub fn epsilon_eff_approx(&self) -> f64 {
        (self.epsilon_r + 1.0) / 2.0
    }
}

pub const FR4_STANDARD: DielectricMaterial = DielectricMaterial {
    name: "FR4",
    epsilon_r: 4.5,
    loss_tangent: 0.02,
    description: "Standard FR4 glass-reinforced epoxy laminate",
};

pub const FR4_HIGH_TG: DielectricMaterial = DielectricMaterial {
    name: "FR4 High-Tg",
    epsilon_r: 4.4,
    loss_tangent: 0.018,
    description: "High glass transition temperature FR4",
};

pub const ROGERS_4350B: DielectricMaterial = DielectricMaterial {
    name: "Rogers RO4350B",
    epsilon_r: 3.48,
    loss_tangent: 0.0037,
    description: "High-frequency laminate with low loss",
};

pub const ROGERS_4003C: DielectricMaterial = DielectricMaterial {
    name: "Rogers RO4003C",
    epsilon_r: 3.55,
    loss_tangent: 0.0027,
    description: "Woven glass reinforced hydrocarbon/ceramic",
};

pub const ISOLA_370HR: DielectricMaterial = DielectricMaterial {
    name: "Isola 370HR",
    epsilon_r: 4.0,
    loss_tangent: 0.015,
    description: "High-performance FR4 alternative",
};

/// Material database by lowercase name.
pub static MATERIALS: &[(&str, &DielectricMaterial)] = &[
    ("fr4", &FR4_STANDARD),
    ("fr-4", &FR4_STANDARD),
    ("fr4 high-tg", &FR4_HIGH_TG),
    ("fr4_high_tg", &FR4_HIGH_TG),
    ("rogers 4350b", &ROGERS_4350B),
    ("ro4350b", &ROGERS_4350B),
    ("rogers 4003c", &ROGERS_4003C),
    ("ro4003c", &ROGERS_4003C),
    ("isola 370hr", &ISOLA_370HR),
    ("370hr", &ISOLA_370HR),
];

/// Case-insensitive material lookup.
pub fn get_material(name: &str) -> Option<&'static DielectricMaterial> {
    let key = name.to_lowercase();
    MATERIALS.iter().find(|(k, _)| *k == key).map(|(_, m)| *m)
}

/// Material lookup returning `default` when absent or unknown.
pub fn get_material_or_default(
    name: Option<&str>,
    default: &'static DielectricMaterial,
) -> &'static DielectricMaterial {
    match name {
        Some(n) if !n.is_empty() => get_material(n).unwrap_or(default),
        _ => default,
    }
}

/// Copper weight (oz/ft^2) to thickness (mm).
pub fn copper_thickness_from_oz(oz: f64) -> f64 {
    oz * 0.035
}

/// Copper thickness (mm) to weight (oz/ft^2); exact inverse of
/// [`copper_thickness_from_oz`].
pub fn copper_oz_from_thickness(thickness_mm: f64) -> f64 {
    thickness_mm / 0.035
}
