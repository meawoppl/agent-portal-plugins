//! Port of `kicad_tools.recovery.types`: failure causes, blocking elements,
//! resolution strategies and side effects.

use crate::pyjson::Json;

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $value:literal),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),* }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),*];

            /// Upstream enum `.value`.
            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $value),* }
            }

            pub fn from_value(v: &str) -> Option<Self> {
                match v { $($value => Some($name::$variant),)* _ => None }
            }
        }
    };
}

str_enum! {
    /// Root causes for routing/placement failures.
    FailureCause {
        Congestion => "congestion",
        BlockedPath => "blocked_path",
        Clearance => "clearance",
        LayerConflict => "layer_conflict",
        PinAccess => "pin_access",
        LengthConstraint => "length_constraint",
        DifferentialPair => "differential_pair",
        Keepout => "keepout",
    }
}

str_enum! {
    /// Types of resolution strategies.
    StrategyType {
        MoveComponent => "move_component",
        MoveMultiple => "move_multiple",
        RotateComponent => "rotate_component",
        MirrorComponent => "mirror_component",
        ReorderPins => "reorder_pins",
        AddVia => "add_via",
        ChangeLayer => "change_layer",
        RerouteNet => "reroute_net",
        RerouteMultiple => "reroute_multiple",
        WidenClearance => "widen_clearance",
        ManualIntervention => "manual_intervention",
    }
}

str_enum! {
    /// Difficulty/risk level of a strategy.
    Difficulty {
        Trivial => "trivial",
        Easy => "easy",
        Medium => "medium",
        Hard => "hard",
        Expert => "expert",
    }
}

/// Axis-aligned bounding box (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rectangle {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Rectangle {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn center(&self) -> (f64, f64) {
        (
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        )
    }

    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        self.min_x <= x && x <= self.max_x && self.min_y <= y && y <= self.max_y
    }

    pub fn intersects(&self, o: &Rectangle) -> bool {
        !(self.max_x < o.min_x
            || self.min_x > o.max_x
            || self.max_y < o.min_y
            || self.min_y > o.max_y)
    }

    pub fn expand(&self, margin: f64) -> Rectangle {
        Rectangle::new(
            self.min_x - margin,
            self.min_y - margin,
            self.max_x + margin,
            self.max_y + margin,
        )
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("min_x", self.min_x);
        d.set("min_y", self.min_y);
        d.set("max_x", self.max_x);
        d.set("max_y", self.max_y);
        d
    }
}

fn opt_str(v: &Option<String>) -> Json {
    v.as_ref().map_or(Json::Null, |s| Json::Str(s.clone()))
}

fn xy(p: (f64, f64)) -> Json {
    let mut d = Json::obj();
    d.set("x", p.0);
    d.set("y", p.1);
    d
}

/// Something blocking the desired operation.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockingElement {
    /// `"component"`, `"trace"`, `"via"`, `"zone"` or `"keepout"`.
    pub kind: String,
    pub reference: Option<String>,
    pub net: Option<String>,
    pub bounds: Rectangle,
    pub movable: bool,
}

impl BlockingElement {
    pub fn new(
        kind: &str,
        reference: Option<&str>,
        net: Option<&str>,
        bounds: Rectangle,
        movable: bool,
    ) -> Self {
        BlockingElement {
            kind: kind.into(),
            reference: reference.map(str::to_string),
            net: net.map(str::to_string),
            bounds,
            movable,
        }
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("type", self.kind.as_str());
        d.set("ref", opt_str(&self.reference));
        d.set("net", opt_str(&self.net));
        d.set("bounds", self.bounds.to_dict());
        d.set("movable", self.movable);
        d
    }
}

/// Record of a routing path attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct PathAttempt {
    pub start: (f64, f64),
    pub end: (f64, f64),
    /// 0-1, how far the path got.
    pub reached: f64,
    pub failure_point: Option<(f64, f64)>,
    pub failure_reason: Option<String>,
}

impl PathAttempt {
    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("start", xy(self.start));
        d.set("end", xy(self.end));
        d.set("reached", self.reached);
        d.set("failure_point", self.failure_point.map_or(Json::Null, xy));
        d.set("failure_reason", opt_str(&self.failure_reason));
        d
    }
}

/// Detailed analysis of why an operation failed.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureAnalysis {
    pub root_cause: FailureCause,
    pub confidence: f64,
    pub failure_location: (f64, f64),
    pub failure_area: Rectangle,
    pub blocking_elements: Vec<BlockingElement>,
    pub attempted_paths: i64,
    pub best_attempt: Option<PathAttempt>,
    pub congestion_score: f64,
    pub clearance_margin: f64,
    pub net: Option<String>,
}

impl FailureAnalysis {
    pub fn new(
        root_cause: FailureCause,
        confidence: f64,
        failure_location: (f64, f64),
        failure_area: Rectangle,
    ) -> Self {
        FailureAnalysis {
            root_cause,
            confidence,
            failure_location,
            failure_area,
            blocking_elements: Vec::new(),
            attempted_paths: 0,
            best_attempt: None,
            congestion_score: 0.0,
            clearance_margin: 0.0,
            net: None,
        }
    }

    pub fn has_movable_blockers(&self) -> bool {
        self.blocking_elements.iter().any(|e| e.movable)
    }

    pub fn has_reroutable_nets(&self) -> bool {
        self.blocking_elements
            .iter()
            .any(|e| e.kind == "trace" && e.net != self.net)
    }

    pub fn near_connector(&self) -> bool {
        const PREFIXES: [&str; 6] = ["J", "CN", "CONN", "USB", "HDMI", "ETH"];
        self.blocking_elements.iter().any(|e| {
            e.reference.as_ref().is_some_and(|r| {
                let u = r.to_uppercase();
                PREFIXES.iter().any(|p| u.starts_with(p))
            })
        })
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("root_cause", self.root_cause.value());
        d.set("confidence", self.confidence);
        d.set("failure_location", xy(self.failure_location));
        d.set("failure_area", self.failure_area.to_dict());
        d.set(
            "blocking_elements",
            Json::Arr(self.blocking_elements.iter().map(|e| e.to_dict()).collect()),
        );
        d.set("attempted_paths", self.attempted_paths);
        d.set(
            "best_attempt",
            self.best_attempt
                .as_ref()
                .map_or(Json::Null, |a| a.to_dict()),
        );
        d.set("congestion_score", self.congestion_score);
        d.set("clearance_margin", self.clearance_margin);
        d.set("net", opt_str(&self.net));
        d
    }
}

/// A potential side effect of a strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct SideEffect {
    pub description: String,
    /// `"info"`, `"warning"` or `"risk"`.
    pub severity: String,
    pub mitigatable: bool,
}

impl SideEffect {
    pub fn new(description: impl Into<String>, severity: &str, mitigatable: bool) -> Self {
        SideEffect {
            description: description.into(),
            severity: severity.into(),
            mitigatable,
        }
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("description", self.description.as_str());
        d.set("severity", self.severity.as_str());
        d.set("mitigatable", self.mitigatable);
        d
    }
}

/// A single action in a strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// `"move"`, `"rotate"`, `"mirror"`, `"add_via"`, `"reroute"`, ...
    pub kind: String,
    pub target: String,
    /// Action parameters (a JSON object, insertion-ordered).
    pub params: Json,
}

impl Action {
    pub fn new(kind: &str, target: impl Into<String>, params: Json) -> Self {
        Action {
            kind: kind.into(),
            target: target.into(),
            params,
        }
    }

    /// `params.get(key)`.
    pub fn param(&self, key: &str) -> Option<&Json> {
        self.params.get(key)
    }

    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("type", self.kind.as_str());
        d.set("target", self.target.as_str());
        d.set("params", self.params.clone());
        d
    }
}

/// A concrete strategy to resolve a failure.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolutionStrategy {
    pub kind: StrategyType,
    pub difficulty: Difficulty,
    pub confidence: f64,
    pub actions: Vec<Action>,
    pub side_effects: Vec<SideEffect>,
    pub affected_components: Vec<String>,
    pub affected_nets: Vec<String>,
    pub estimated_improvement: f64,
}

impl ResolutionStrategy {
    pub fn new(kind: StrategyType, difficulty: Difficulty, confidence: f64) -> Self {
        ResolutionStrategy {
            kind,
            difficulty,
            confidence,
            actions: Vec::new(),
            side_effects: Vec::new(),
            affected_components: Vec::new(),
            affected_nets: Vec::new(),
            estimated_improvement: 0.0,
        }
    }

    pub fn to_dict(&self) -> Json {
        let strs = |v: &[String]| Json::Arr(v.iter().map(|s| Json::Str(s.clone())).collect());
        let mut d = Json::obj();
        d.set("type", self.kind.value());
        d.set("difficulty", self.difficulty.value());
        d.set("confidence", self.confidence);
        d.set(
            "actions",
            Json::Arr(self.actions.iter().map(|a| a.to_dict()).collect()),
        );
        d.set(
            "side_effects",
            Json::Arr(self.side_effects.iter().map(|e| e.to_dict()).collect()),
        );
        d.set("affected_components", strs(&self.affected_components));
        d.set("affected_nets", strs(&self.affected_nets));
        d.set("estimated_improvement", self.estimated_improvement);
        d
    }
}
