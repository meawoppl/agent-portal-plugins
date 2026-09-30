use super::{parse_args, Globals};
use crate::footprints::{self, Footprint};
use anyhow::Result;
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Generate(Generate),
}
#[derive(ClapArgs)]
struct Generate {
    #[arg(long)]
    list: bool,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    generator: Option<Generator>,
}
#[derive(Subcommand)]
enum Generator {
    Soic {
        #[arg(long)]
        pins: usize,
        #[arg(long)]
        pitch: Option<f64>,
        #[arg(long)]
        body_width: Option<f64>,
        #[arg(long)]
        body_length: Option<f64>,
        #[command(flatten)]
        out: Output,
    },
    Qfp {
        #[arg(long)]
        pins: usize,
        #[arg(long)]
        pitch: Option<f64>,
        #[arg(long)]
        body_size: Option<f64>,
        #[command(flatten)]
        out: Output,
    },
    Qfn {
        #[arg(long)]
        pins: usize,
        #[arg(long)]
        pitch: Option<f64>,
        #[arg(long)]
        body_size: Option<f64>,
        #[arg(long)]
        exposed_pad: Option<f64>,
        #[command(flatten)]
        out: Output,
    },
    Chip {
        #[arg(long)]
        size: String,
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long)]
        metric: bool,
        #[command(flatten)]
        out: Output,
    },
    Sot {
        #[arg(long)]
        variant: String,
        #[command(flatten)]
        out: Output,
    },
    Dip {
        #[arg(long)]
        pins: usize,
        #[arg(long)]
        pitch: Option<f64>,
        #[arg(long)]
        row_spacing: Option<f64>,
        #[command(flatten)]
        out: Output,
    },
    PinHeader {
        #[arg(long)]
        pins: usize,
        #[arg(long, default_value_t = 1)]
        rows: usize,
        #[arg(long)]
        pitch: Option<f64>,
        #[command(flatten)]
        out: Output,
    },
}
#[derive(ClapArgs)]
struct Output {
    #[arg(long)]
    name: Option<String>,
    #[arg(short, long)]
    output: Option<PathBuf>,
}

pub fn run(args: Vec<OsString>, _globals: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("footprint", args);
    match args.command {
        Command::Generate(g) => run_generate(g),
    }
}
fn run_generate(g: Generate) -> Result<i32> {
    if g.list {
        println!("soic\nqfp\nqfn\nchip\nsot\ndip\npin-header");
        return Ok(0);
    }
    let gen = g
        .generator
        .context("footprint generate requires a generator or --list")?;
    let (fp, out) = match gen {
        Generator::Soic {
            pins,
            pitch,
            body_width,
            body_length,
            out,
        } => (
            footprints::soic(pins, pitch, body_width, body_length, out.name.clone())?,
            out,
        ),
        Generator::Qfp {
            pins,
            pitch,
            body_size,
            out,
        } => (
            footprints::qfp(pins, pitch, body_size, out.name.clone())?,
            out,
        ),
        Generator::Qfn {
            pins,
            pitch,
            body_size,
            exposed_pad,
            out,
        } => (
            footprints::qfn(pins, pitch, body_size, exposed_pad, out.name.clone())?,
            out,
        ),
        Generator::Chip {
            size,
            prefix,
            metric,
            out,
        } => (
            footprints::chip(&size, &prefix, metric, out.name.clone())?,
            out,
        ),
        Generator::Sot { variant, out } => (footprints::sot(&variant, out.name.clone())?, out),
        Generator::Dip {
            pins,
            pitch,
            row_spacing,
            out,
        } => (
            footprints::dip(pins, pitch, row_spacing, out.name.clone())?,
            out,
        ),
        Generator::PinHeader {
            pins,
            rows,
            pitch,
            out,
        } => (
            footprints::pin_header(pins, rows, pitch, out.name.clone())?,
            out,
        ),
    };
    output(fp, out, g.json)
}
fn output(fp: Footprint, out: Output, json: bool) -> Result<i32> {
    if json {
        println!("{}", serde_json::to_string_pretty(&fp)?)
    } else if let Some(mut path) = out.output {
        if path.extension().is_none() {
            path.set_extension("kicad_mod");
        }
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        crate::fsutil::atomic_write(&path, fp.to_sexp().as_bytes())?;
        println!("Saved: {}", path.display())
    } else {
        print!("{}", fp.to_sexp())
    }
    Ok(0)
}

trait ContextOption<T> {
    fn context(self, msg: &str) -> Result<T>;
}
impl<T> ContextOption<T> for Option<T> {
    fn context(self, msg: &str) -> Result<T> {
        self.ok_or_else(|| anyhow::anyhow!(msg.to_owned()))
    }
}
