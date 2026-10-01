//! Native placement inspection and deterministic editing tools.

use std::ffi::OsString;

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Check {
        pcb: PathBuf,
        #[arg(long = "pad-clearance", default_value_t = 0.2)]
        pad_clearance: f64,
        #[arg(long,default_value="table",value_parser=["table","json","summary"])]
        format: String,
    },
    Snap(Snap),
    Align(Align),
    Distribute(Distribute),
    Suggest {
        pcb: PathBuf,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Fix {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Nudge {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Optimize {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Refine {
        pcb: PathBuf,
    },
}
#[derive(ClapArgs)]
struct Snap {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 0.5)]
    grid: f64,
    #[arg(long, default_value_t = 90.0)]
    rotation: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}
#[derive(ClapArgs)]
struct Align {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_delimiter = ',')]
    components: Vec<String>,
    #[arg(long,default_value="row",value_parser=["row","column"])]
    axis: String,
    #[arg(long, default_value = "center")]
    reference: String,
    #[arg(long, default_value_t = 0.1)]
    tolerance: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}
#[derive(ClapArgs)]
struct Distribute {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_delimiter = ',')]
    components: Vec<String>,
    #[arg(long,default_value="horizontal",value_parser=["horizontal","vertical"])]
    direction: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

fn require_output(output: &Option<PathBuf>, dry: bool) -> Result<()> {
    if !dry && output.is_none() {
        bail!("placement edits require --output (or use --dry-run)")
    }
    Ok(())
}
fn radius(fp: &crate::schema::pcb::Footprint) -> f64 {
    fp.pads
        .iter()
        .map(|p| p.position.0.abs().max(p.position.1.abs()) + p.size.0.max(p.size.1) / 2.0)
        .fold(0.5, f64::max)
}
fn conflicts(pcb: &Pcb, clearance: f64) -> Vec<serde_json::Value> {
    let f = pcb.footprints();
    let mut out = Vec::new();
    for i in 0..f.len() {
        for b in &f[i + 1..] {
            let a = &f[i];
            let actual = (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1)
                - radius(a)
                - radius(b);
            if actual < clearance {
                out.push(serde_json::json!({"type":"component_overlap","component1":a.reference,"component2":b.reference,"clearance_mm":actual.max(0.0),"required_mm":clearance,"severity":"error"}));
            }
        }
    }
    out
}
fn emit(format: &str, value: &serde_json::Value, text: &str) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(value)?)
    } else {
        println!("{text}")
    }
    Ok(())
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("placement", args);
    match args.command {
        Command::Check{pcb,pad_clearance,format}=>{let board=Pcb::load(&pcb)?;let c=conflicts(&board,pad_clearance);let result=serde_json::json!({"pcb":pcb,"components":board.footprints().len(),"conflicts":c,"conflict_count":c.len(),"passed":c.is_empty(),"method":"conservative component pad-envelope circles"});emit(&format,&result,&format!("Placement: {} component(s), {} conflict(s)",board.footprints().len(),c.len()))?;return Ok(if c.is_empty(){0}else{1})}
        Command::Snap(a)=>{require_output(&a.output,a.dry_run)?;if a.grid<=0.0||a.rotation<0.0{bail!("grid must be > 0 and rotation >= 0")}let mut board=Pcb::load(&a.pcb)?;let changes:Vec<_>=board.footprints().iter().filter(|f|!f.locked).map(|f|{let p=((f.position.0/a.grid).round()*a.grid,(f.position.1/a.grid).round()*a.grid);let r=if a.rotation>0.0{(f.rotation/a.rotation).round()*a.rotation}else{f.rotation};(f.reference.clone(),f.position,p,f.rotation,r)}).filter(|x|x.1!=x.2||x.3!=x.4).collect();if !a.dry_run{for (r,_,p,_,rot) in &changes{let mut fp=board.footprint_mut(r).with_context(||format!("footprint {r} disappeared"))?;fp.set_position(*p);fp.set_rotation(*rot);}board.save(a.output.as_deref())?;}let result=serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"grid_mm":a.grid,"rotation_degrees":a.rotation,"components_updated":changes.len(),"changes":changes.iter().map(|(r,old,new,ro,rn)|serde_json::json!({"reference":r,"from":[old.0,old.1,ro],"to":[new.0,new.1,rn]})).collect::<Vec<_>>()});emit(&a.format,&result,&format!("{} {} component(s)",if a.dry_run{"Would snap"}else{"Snapped"},changes.len()))?;}
        Command::Align(a)=>{require_output(&a.output,a.dry_run)?;let mut board=Pcb::load(&a.pcb)?;let refs:BTreeSet<_>=a.components.iter().cloned().collect();let selected:Vec<_>=board.footprints().iter().filter(|f|refs.contains(&f.reference)).map(|f|(f.reference.clone(),f.position)).collect();if selected.len()<2{bail!("select at least two existing components")};let target=if a.axis=="row"{selected.iter().map(|x|x.1.1).sum::<f64>()/selected.len() as f64}else{selected.iter().map(|x|x.1.0).sum::<f64>()/selected.len() as f64};let changes:Vec<_>=selected.into_iter().filter_map(|(r,p)|{let n=if a.axis=="row"{(p.0,target)}else{(target,p.1)};if (p.0-n.0).abs().max((p.1-n.1).abs())>a.tolerance{Some((r,p,n))}else{None}}).collect();if !a.dry_run{for (r,_,p) in &changes{board.footprint_mut(r).unwrap().set_position(*p)}board.save(a.output.as_deref())?}let result=serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"axis":a.axis,"reference":a.reference,"target":target,"components_updated":changes.len(),"changes":changes});emit(&a.format,&result,&format!("{} {} component(s)",if a.dry_run{"Would align"}else{"Aligned"},changes.len()))?;}
        Command::Distribute(a)=>{require_output(&a.output,a.dry_run)?;let mut board=Pcb::load(&a.pcb)?;let refs:BTreeSet<_>=a.components.iter().cloned().collect();let mut selected:Vec<_>=board.footprints().iter().filter(|f|refs.contains(&f.reference)).map(|f|(f.reference.clone(),f.position)).collect();if selected.len()<3{bail!("select at least three existing components")};let axis=if a.direction=="horizontal"{0}else{1};selected.sort_by(|x,y|if axis==0{x.1.0.total_cmp(&y.1.0)}else{x.1.1.total_cmp(&y.1.1)});let lo=if axis==0{selected[0].1.0}else{selected[0].1.1};let hi=if axis==0{selected.last().unwrap().1.0}else{selected.last().unwrap().1.1};let step=(hi-lo)/(selected.len()-1) as f64;let changes:Vec<_>=selected.into_iter().enumerate().map(|(i,(r,p))|{let n=if axis==0{(lo+i as f64*step,p.1)}else{(p.0,lo+i as f64*step)};(r,p,n)}).collect();if !a.dry_run{for (r,_,p) in &changes{board.footprint_mut(r).unwrap().set_position(*p)}board.save(a.output.as_deref())?}let result=serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"direction":a.direction,"spacing_mm":step,"components_updated":changes.len(),"changes":changes});emit(&a.format,&result,&format!("{} {} component(s)",if a.dry_run{"Would distribute"}else{"Distributed"},changes.len()))?;}
        Command::Suggest{pcb,format}=>{let board=Pcb::load(&pcb)?;let c=conflicts(&board,0.2);let suggestions:Vec<_>=c.iter().map(|v|serde_json::json!({"action":"separate","components":[v["component1"].clone(),v["component2"].clone()],"rationale":"component pad envelopes overlap or violate the requested floor","advisory":true})).collect();let result=serde_json::json!({"pcb":pcb,"suggestions":suggestions,"count":suggestions.len(),"authorized_edits":false});emit(&format,&result,&format!("Generated {} advisory placement suggestion(s)",suggestions.len()))?;}
        Command::Fix{pcb,output,dry_run,format}|Command::Nudge{pcb,output,dry_run,format}|Command::Optimize{pcb,output,dry_run,format}=>{require_output(&output,dry_run)?;let board=Pcb::load(&pcb)?;let c=conflicts(&board,0.2);let result=serde_json::json!({"pcb":pcb,"output":output,"dry_run":dry_run,"conflicts":c,"components_updated":0,"success":c.is_empty(),"message":"No automatic move was made: use placement suggest, align, distribute, or snap for auditable deterministic edits."});emit(&format,&result,result["message"].as_str().unwrap())?;return Ok(if c.is_empty(){0}else{1})}
        Command::Refine{pcb}=>bail!("interactive refinement is intentionally unavailable in the single binary; use the workbench on {}",pcb.display()),
    }
    Ok(0)
}
