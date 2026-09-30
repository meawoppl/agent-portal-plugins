use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use regex::Regex;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Search {
        part: String,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Download {
        part: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    List {
        #[arg(long, default_value = "text")]
        format: String,
    },
    Cache {
        #[arg(long)]
        clear: bool,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Convert {
        pdf: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        pages: Option<String>,
    },
    ExtractImages {
        pdf: PathBuf,
        #[arg(short, long, default_value = "images")]
        output: PathBuf,
        #[arg(long)]
        pages: Option<String>,
        #[arg(long)]
        min_width: Option<u32>,
        #[arg(long)]
        min_height: Option<u32>,
    },
    ExtractTables {
        pdf: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "markdown")]
        format: String,
        #[arg(long)]
        pages: Option<String>,
    },
    ExtractPins {
        pdf: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        pages: Option<String>,
    },
    Info {
        pdf: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    AnalyzePackage {
        pdf: PathBuf,
        #[arg(long, default_value = "json")]
        format: String,
    },
    SuggestFootprint {
        pdf: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    GenerateSymbol {
        pdf: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "simple")]
        layout: String,
    },
}
fn dir() -> PathBuf {
    crate::parts::cache_dir()
        .parent()
        .unwrap_or(Path::new("."))
        .join("datasheets")
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Args>("datasheet", args).command {
        Command::Search { part, format } => {
            let p = crate::parts::cache_get(&part, 7)?
                .or(crate::parts::lookup(&part)?)
                .context("part not found")?;
            let data = serde_json::json!({"part":p.lcsc_part,"manufacturer_part":p.mfr_part,"datasheet_url":p.datasheet_url});
            emit(
                &format,
                &data,
                format!("{}\n{}", p.mfr_part, p.datasheet_url),
            )?
        }
        Command::Download {
            part,
            output,
            force,
        } => {
            let p = crate::parts::cache_get(&part, 7)?
                .or(crate::parts::lookup(&part)?)
                .context("part not found")?;
            if p.datasheet_url.is_empty() {
                bail!("part has no datasheet URL")
            }
            let out = output.unwrap_or_else(|| dir().join(format!("{}.pdf", p.lcsc_part)));
            if out.exists() && !force {
                println!("{}", out.display())
            } else {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?
                }
                let bytes = reqwest::blocking::get(&p.datasheet_url)?
                    .error_for_status()?
                    .bytes()?;
                crate::fsutil::atomic_write(&out, &bytes)?;
                println!("{}", out.display())
            }
        }
        Command::List { format } => {
            let d = dir();
            let files = if d.exists() {
                std::fs::read_dir(&d)?
                    .filter_map(|e| e.ok().map(|x| x.path()))
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("pdf"))
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            let data = serde_json::json!({"cache":d,"files":files});
            emit(
                &format,
                &data,
                files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?
        }
        Command::Cache { clear, format } => {
            let d = dir();
            if clear && d.exists() {
                std::fs::remove_dir_all(&d)?
            }
            let count = if d.exists() {
                std::fs::read_dir(&d)?.count()
            } else {
                0
            };
            let data = serde_json::json!({"cache":d,"count":count,"cleared":clear});
            emit(
                &format,
                &data,
                format!("{count} cached datasheet(s) in {}", d.display()),
            )?
        }
        Command::Convert {
            pdf,
            output,
            pages: _,
        } => {
            let text = pdf_extract::extract_text(&pdf)?;
            let markdown = format!(
                "# {}\n\n{}\n",
                pdf.file_stem().unwrap_or_default().to_string_lossy(),
                text
            );
            write_or_print(output, &markdown)?
        }
        Command::Info { pdf, format } => {
            let meta = std::fs::metadata(&pdf)?;
            let text = pdf_extract::extract_text(&pdf)?;
            let data = serde_json::json!({"path":pdf,"bytes":meta.len(),"characters":text.len(),"page_markers":text.matches('\u{c}').count()+1});
            emit(
                &format,
                &data,
                format!(
                    "{}\n{} bytes\n{} extracted characters",
                    pdf.display(),
                    meta.len(),
                    text.len()
                ),
            )?
        }
        Command::ExtractPins {
            pdf,
            output,
            format,
            pages: _,
        } => {
            let text = pdf_extract::extract_text(&pdf)?;
            let re = Regex::new(r"(?im)^\s*(\d{1,4})\s+([A-Z][A-Z0-9_/#.+-]{0,30})\s+(.+)$")?;
            let pins = re
                .captures_iter(&text)
                .map(|c| serde_json::json!({"number":&c[1],"name":&c[2],"description":c[3].trim()}))
                .collect::<Vec<_>>();
            let s = if format == "json" {
                serde_json::to_string_pretty(&pins)?
            } else {
                pins.iter()
                    .map(|p| {
                        format!(
                            "| {} | {} | {} |",
                            p["number"].as_str().unwrap(),
                            p["name"].as_str().unwrap(),
                            p["description"].as_str().unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            write_or_print(output, &s)?
        }
        Command::ExtractTables {
            pdf,
            output,
            format: _,
            pages: _,
        } => {
            let text = pdf_extract::extract_text(&pdf)?;
            let rows = text
                .lines()
                .filter(|l| l.split_whitespace().count() >= 3)
                .map(|l| {
                    format!(
                        "| {} |",
                        l.split_whitespace().collect::<Vec<_>>().join(" | ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            write_or_print(output, &rows)?
        }
        Command::ExtractImages {
            pdf,
            output,
            pages: _,
            min_width: _,
            min_height: _,
        } => {
            std::fs::create_dir_all(&output)?;
            let data = std::fs::read(&pdf)?;
            let count = extract_jpegs(&data, &output)?;
            println!("extracted {count} JPEG stream(s) to {}", output.display())
        }
        Command::AnalyzePackage { pdf, format } | Command::SuggestFootprint { pdf, format } => {
            let text = pdf_extract::extract_text(&pdf)?.to_ascii_uppercase();
            let packages = [
                "BGA", "QFN", "DFN", "LQFP", "TQFP", "SOIC", "TSSOP", "SOT-23", "DIP",
            ]
            .into_iter()
            .filter(|p| text.contains(p))
            .collect::<Vec<_>>();
            let data = serde_json::json!({"packages":packages,"source":pdf});
            emit(&format, &data, packages.join("\n"))?
        }
        Command::GenerateSymbol {
            pdf,
            output,
            name,
            layout: _,
        } => {
            let text = pdf_extract::extract_text(&pdf)?;
            let re = Regex::new(r"(?im)^\s*(\d{1,4})\s+([A-Z][A-Z0-9_/#.+-]{0,30})")?;
            let pins = re
                .captures_iter(&text)
                .take(512)
                .map(|c| (c[1].to_owned(), c[2].to_owned()))
                .collect::<Vec<_>>();
            let n = name.unwrap_or_else(|| {
                pdf.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let mut sym = format!(
                "(kicad_symbol_lib (version 20231120) (generator kicadmium)\n  (symbol {:?}\n",
                n
            );
            for (number, pin) in pins {
                sym.push_str(&format!("    (symbol {:?} (pin_names (offset 0)) (in_bom yes) (on_board yes) (property \"Reference\" \"U\" (at 0 0 0)) (pin input line (at 0 0 0) (length 2.54) (name {:?}) (number {:?})))\n",format!("{n}_{number}"),pin,number));
            }
            sym.push_str("  )\n)\n");
            crate::fsutil::atomic_write(&output, sym.as_bytes())?;
            println!("{}", output.display())
        }
    }
    Ok(0)
}
fn emit(format: &str, data: &serde_json::Value, text: String) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(data)?)
    } else {
        println!("{text}")
    }
    Ok(())
}
fn write_or_print(output: Option<PathBuf>, s: &str) -> Result<()> {
    if let Some(p) = output {
        crate::fsutil::atomic_write(&p, s.as_bytes())?
    } else {
        print!("{s}")
    }
    Ok(())
}
fn extract_jpegs(data: &[u8], out: &Path) -> Result<usize> {
    let mut from = 0;
    let mut n = 0;
    while let Some(start) = data[from..]
        .windows(2)
        .position(|w| w == [0xff, 0xd8])
        .map(|i| i + from)
    {
        let Some(end) = data[start + 2..]
            .windows(2)
            .position(|w| w == [0xff, 0xd9])
            .map(|i| i + start + 4)
        else {
            break;
        };
        n += 1;
        crate::fsutil::atomic_write(&out.join(format!("image-{n:03}.jpg")), &data[start..end])?;
        from = end;
    }
    Ok(n)
}
