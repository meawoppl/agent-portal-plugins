//! Deterministic native SVG/PNG region rendering from the workbench scene.
use anyhow::{bail, Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use vector_view::{Prim, Role, Scene};
#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    project: Option<String>,
    /// Region in millimetres: x y width height.
    #[arg(long, num_args = 4, allow_hyphen_values = true)]
    region: Vec<f64>,
    #[arg(long, value_delimiter = ',', default_value = "F.Cu,F.SilkS,Edge.Cuts")]
    layers: Vec<String>,
    #[arg(long, default_value = "filled")]
    zones: String,
    #[arg(long)]
    baseline: Option<String>,
    #[arg(long, value_delimiter = ',')]
    highlight_nets: Vec<String>,
    #[arg(long)]
    annotation: bool,
    #[arg(short, long)]
    output: PathBuf,
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn path(points: &[[f64; 2]], closed: bool) -> String {
    let mut s = String::new();
    for (i, p) in points.iter().enumerate() {
        s += &format!("{}{} {}", if i == 0 { "M" } else { "L" }, p[0], p[1]);
    }
    if closed {
        s.push('Z')
    }
    s
}
fn primitive(p: &Prim, color: &str) -> String {
    match p{
 Prim::Polyline{points,width}=>format!("<path d=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{width}\"/>",path(points,false)),
 Prim::Polygon{outer,holes,fill,stroke}=>{let mut d=path(outer,true);for h in holes{d+=&path(h,true)}format!("<path d=\"{d}\" fill=\"{}\" fill-rule=\"evenodd\" stroke=\"{color}\" stroke-width=\"{stroke}\"/>",if *fill{color}else{"none"})},
 Prim::Circle{center,radius,fill,stroke}=>format!("<circle cx=\"{}\" cy=\"{}\" r=\"{radius}\" fill=\"{}\" stroke=\"{color}\" stroke-width=\"{stroke}\"/>",center[0],center[1],if *fill{color}else{"none"}),
 Prim::Arc{center,radius,start,end,width}=>{let sweep=(end-start).rem_euclid(std::f64::consts::TAU);let points:Vec<_>=(0..=96).map(|i|{let a=start+sweep*f64::from(i)/96.;[center[0]+radius*a.cos(),center[1]+radius*a.sin()]}).collect();primitive(&Prim::Polyline{points,width:*width},color)},
 Prim::Strokes{strokes,width}=>strokes.iter().map(|points|primitive(&Prim::Polyline{points:points.clone(),width:*width},color)).collect(),
 Prim::Hole{center,size,rotation}=>format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"#1a1b26\" transform=\"rotate({} {} {})\"/>",center[0]-size[0]/2.,center[1]-size[1]/2.,size[0],size[1],size[0].min(size[1])/2.,rotation.to_degrees(),center[0],center[1])}
}
fn panel(scene: &Scene, a: &Args, x: f64, label: &str, width: f64, height: f64) -> String {
    let [rx, ry, rw, rh] = a.region.as_slice() else {
        unreachable!()
    };
    let mut out=format!("<text x=\"{}\" y=\"24\" fill=\"#c0caf5\" font-size=\"18\">{}</text><svg x=\"{x}\" y=\"36\" width=\"{width}\" height=\"{height}\" viewBox=\"{rx} {ry} {rw} {rh}\" overflow=\"hidden\"><g stroke-linecap=\"round\" stroke-linejoin=\"round\">",x+12.,escape(label));
    let mut layers = scene
        .layers
        .iter()
        .filter(|l| a.layers.contains(&l.name) || l.kind == vector_view::LayerKind::Drill)
        .collect::<Vec<_>>();
    layers.sort_by_key(|l| l.z);
    for l in layers {
        for item in scene.items.iter().filter(|i| i.layer == l.id) {
            let net = scene
                .nets
                .iter()
                .find(|n| Some(n.id) == item.net)
                .map(|n| n.name.as_str())
                .unwrap_or("");
            if item.role == Role::Zone
                && (a.zones == "none" || a.zones.strip_prefix("net=").is_some_and(|n| n != net))
            {
                continue;
            }
            let b = item.prim.bbox();
            if b.max[0] < *rx || b.min[0] > rx + rw || b.max[1] < *ry || b.min[1] > ry + rh {
                continue;
            }
            let color = if a.highlight_nets.iter().any(|n| n == net) {
                "#e0af68"
            } else {
                match l.kind {
                    vector_view::LayerKind::Copper => {
                        if l.name.starts_with('B') {
                            "#7aa2f7"
                        } else if l.name.starts_with("In") {
                            "#bb9af7"
                        } else {
                            "#9ece6a"
                        }
                    }
                    vector_view::LayerKind::Silkscreen => "#c0caf5",
                    vector_view::LayerKind::EdgeCuts => "#7dcfff",
                    _ => "#565f89",
                }
            };
            out += &primitive(&item.prim, color);
        }
    }
    if a.annotation {
        out+=&format!("<rect x=\"{rx}\" y=\"{ry}\" width=\"{rw}\" height=\"{rh}\" fill=\"none\" stroke=\"#f7768e\" stroke-width=\"{}\"/>",rw/300.);
    }
    out + "</g></svg>"
}
fn baseline(project: &crate::ProjectContext, board: &Path, revision: &str) -> Result<String> {
    if Path::new(revision).is_file() {
        return Ok(std::fs::read_to_string(revision)?);
    }
    let commit = Command::new("git")
        .arg("-C")
        .arg(&project.repo_root)
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .output()?;
    if !commit.status.success() {
        bail!("invalid baseline revision")
    }
    let commit = String::from_utf8(commit.stdout)?;
    let relative = board.strip_prefix(&project.repo_root)?;
    let result = Command::new("git")
        .arg("-C")
        .arg(&project.repo_root)
        .args(["show", &format!("{}:{}", commit.trim(), relative.display())])
        .output()?;
    if !result.status.success() {
        bail!("board is absent from baseline")
    };
    Ok(String::from_utf8(result.stdout)?)
}
pub(crate) fn run(a: Args) -> Result<()> {
    if a.region.len() != 4
        || !a.region.iter().all(|v| v.is_finite())
        || a.region[2] <= 0.
        || a.region[3] <= 0.
    {
        bail!("region must be finite x y positive-width positive-height")
    }
    if a.zones != "none"
        && a.zones != "filled"
        && !a.zones.strip_prefix("net=").is_some_and(|n| !n.is_empty())
    {
        bail!("zones must be none, filled, or net=NAME")
    }
    let project = crate::cli_project_context(&a.cwd.canonicalize()?, a.project.as_deref())?;
    let board = crate::pick_project_file(&project, "kicad_pcb")?.context("project has no PCB")?;
    let source = std::fs::read_to_string(&board)?;
    let after = crate::pcb_scene::build_scene(&source, "working", "board")?;
    for layer in &a.layers {
        if !after.layers.iter().any(|l| l.name == *layer) {
            bail!("unknown rendered layer {layer}")
        }
    }
    let before = a
        .baseline
        .as_deref()
        .map(|r| baseline(&project, &board, r))
        .transpose()?
        .map(|s| crate::pcb_scene::build_scene(&s, "baseline", "board"))
        .transpose()?;
    let width = 800.;
    let height = (width * a.region[3] / a.region[2]).clamp(100., 1600.);
    let total = if before.is_some() { width * 2. } else { width };
    let mut svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{total}\" height=\"{}\"><rect width=\"100%\" height=\"100%\" fill=\"#1a1b26\"/>",height+36.);
    if let Some(before) = before {
        svg += &panel(&before, &a, 0., "Before", width, height);
        svg += &panel(&after, &a, width, "After", width, height);
    } else {
        svg += &panel(&after, &a, 0., "Working board", width, height);
    }
    svg += "</svg>";
    match a.output.extension().and_then(|s| s.to_str()) {
        Some("svg") => std::fs::write(&a.output, svg)?,
        Some("png") => {
            let mut options = resvg::usvg::Options::default();
            options.fontdb_mut().load_system_fonts();
            let tree = resvg::usvg::Tree::from_str(&svg, &options)?;
            let size = tree.size().to_int_size();
            let mut pix = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
                .context("allocate render")?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::identity(),
                &mut pix.as_mut(),
            );
            pix.save_png(&a.output)?;
        }
        _ => bail!("output must be .svg or .png"),
    }
    println!("{}", a.output.display());
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn polygon_holes_are_preserved() {
        let p = Prim::Polygon {
            outer: vec![[0., 0.], [10., 0.], [10., 10.]],
            holes: vec![vec![[1., 1.], [2., 1.], [2., 2.]]],
            fill: true,
            stroke: 0.,
        };
        let svg = primitive(&p, "#9ece6a");
        assert!(svg.contains("evenodd"));
        assert_eq!(svg.matches('Z').count(), 2);
    }
}
