use super::*;

pub(crate) fn role(path: &Path) -> Option<&'static str> {
    if !path.extension()?.to_str()?.eq_ignore_ascii_case("csv") {
        return None;
    }
    let name = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let tokens: Vec<_> = name.split(['-', '_', ' ']).collect();
    if tokens
        .iter()
        .any(|v| matches!(*v, "cpl" | "pos" | "position" | "placement"))
    {
        return Some("Placement");
    }
    if tokens.contains(&"bom") {
        return Some("BOM");
    }
    None
}

fn cell(value: &str, header: &str) -> String {
    let text = escape(value);
    let id = value.trim();
    if header.to_ascii_lowercase().contains("lcsc")
        && id.starts_with('C')
        && id.len() > 1
        && id[1..].bytes().all(|c| c.is_ascii_digit())
    {
        format!("<a href='https://www.lcsc.com/product-detail/{id}.html' target='_blank' rel='noopener noreferrer'>{text}</a>")
    } else {
        text
    }
}

fn table(path: &Path) -> Result<String> {
    if path.metadata()?.len() > 2_000_000 {
        return Ok("<p>CSV is too large to preview. Download it to view all rows.</p>".into());
    }
    let mut reader = csv::ReaderBuilder::new().from_path(path)?;
    let headers = reader.headers()?.clone();
    if headers.is_empty() {
        return Ok("<p>Empty CSV.</p>".into());
    }
    let mut html = String::from(
        "<div style='overflow:auto;max-height:65vh'><table class='bom-table'><thead><tr>",
    );
    for h in &headers {
        html.push_str(&format!("<th>{}</th>", escape(h)));
    }
    html.push_str("</tr></thead><tbody>");
    let mut count = 0;
    let mut truncated = false;
    for record in reader.records().take(501) {
        let record = record?;
        if count == 500 {
            truncated = true;
            break;
        }
        count += 1;
        html.push_str("<tr>");
        for (i, v) in record.iter().enumerate() {
            html.push_str(&format!(
                "<td>{}</td>",
                cell(v, headers.get(i).unwrap_or(""))
            ));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table></div>");
    html.push_str(&format!(
        "<p class='muted'>{count} rows{}.</p>",
        if truncated {
            " shown; download for the complete CSV"
        } else {
            ""
        }
    ));
    Ok(html)
}

pub(crate) fn render(project: &ProjectContext) -> Result<String> {
    // Read discovery afresh: CSV creation/updates must not depend on a KiCad source revision.
    let mut files = detect_files(&project.root)?;
    files.retain(|f| {
        role(Path::new(&f.path)).is_some()
            && is_project_file(&project.root, &project.root.join(&f.path), true)
    });
    files.sort_by_key(|f| {
        (
            role(Path::new(&f.path)) != Some("BOM"),
            !f.path.starts_with("fab/bom/"),
            !f.path.contains("jlcpcb"),
            f.path.clone(),
        )
    });
    if files.is_empty() {
        return Ok("<p class='muted'>No BOM/assembly artifacts found. Generate a BOM or CPL CSV for this board.</p>".into());
    }
    let mut html = String::new();
    for f in files {
        let path = match safe_rel(&project.root, &f.path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let title = role(Path::new(&f.path)).unwrap();
        let href = format!(
            "/api/kicad/file?project={}&amp;path={}&amp;download=1",
            percent_encode(&project.id),
            percent_encode(&f.path)
        );
        html.push_str(&format!(
            "<article class='card'><h3>{title}</h3><p><a href='{href}'>Download {}</a></p>",
            escape(&f.path)
        ));
        match table(&path) {
            Ok(t) => html.push_str(&t),
            Err(e) => html.push_str(&format!(
                "<p>Unable to preview CSV: {}</p>",
                escape(&e.to_string())
            )),
        }
        html.push_str("</article>");
    }
    Ok(html)
}

pub(crate) async fn endpoint(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Html<String>, AppError> {
    let project = selected_project(&state, query.project.as_deref())?;
    Ok(Html(render(&project)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_excludes_unrelated_csvs() {
        assert_eq!(role(Path::new("docs/carrier-pinout.csv")), None);
        assert_eq!(role(Path::new("fab/bom/pcb-features.csv")), None);
        assert_eq!(role(Path::new("fab/bom/module-bom.csv")), Some("BOM"));
        assert_eq!(
            role(Path::new("fab/jlcpcb/CPL_module.csv")),
            Some("Placement")
        );
    }
    #[test]
    fn quoted_csv_and_untrusted_cells() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bom.csv");
        std::fs::write(
            &p,
            "Designators,LCSC,Notes\n\"R1,R2\",C123,\"<script>\ntext\"\n",
        )
        .unwrap();
        let h = table(&p).unwrap();
        assert!(h.contains("R1,R2"));
        assert!(h.contains("C123.html"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(!h.contains("<script>"));
    }
}
