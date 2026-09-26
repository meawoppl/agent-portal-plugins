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
    if is_designator_header(header) {
        return designator_cell(value);
    }
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

fn is_designator_header(header: &str) -> bool {
    let h = header.trim().to_ascii_lowercase();
    matches!(
        h.as_str(),
        "designator" | "designators" | "reference" | "references" | "ref" | "refs"
    )
}

fn designator_cell(value: &str) -> String {
    let compact = compact_designators(value);
    let formatted = wrap_commas(&compact, 5);
    if compact == value.trim() {
        formatted
    } else {
        format!(
            "<span title='{}'>{formatted}</span>",
            escape_attr(value.trim())
        )
    }
}

fn compact_designators(value: &str) -> String {
    let mut groups: Vec<(String, Vec<u32>)> = Vec::new();
    let mut leftovers = Vec::new();
    for part in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
        if let Some((prefix, number)) = split_designator(part) {
            if let Some((_, nums)) = groups.iter_mut().find(|(p, _)| p == &prefix) {
                nums.push(number);
            } else {
                groups.push((prefix, vec![number]));
            }
        } else {
            leftovers.push(part.to_string());
        }
    }

    let parsed_count = groups.iter().map(|(_, nums)| nums.len()).sum::<usize>();
    if parsed_count < 6 || !leftovers.is_empty() {
        return value.trim().to_string();
    }

    let mut parts = Vec::new();
    for (prefix, mut nums) in groups {
        nums.sort_unstable();
        nums.dedup();
        parts.push(format!("{prefix}({})", compact_ranges(&nums)));
    }
    parts.join(", ")
}

fn split_designator(value: &str) -> Option<(String, u32)> {
    let split_at = value
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);
    if split_at == value.len() {
        return None;
    }
    let (prefix, number) = value.split_at(split_at);
    if prefix.is_empty()
        || !prefix.chars().all(|c| c.is_ascii_alphabetic())
        || number.is_empty()
        || (number.len() > 1 && number.starts_with('0'))
    {
        return None;
    }
    Some((prefix.to_string(), number.parse().ok()?))
}

fn compact_ranges(nums: &[u32]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < nums.len() {
        let start = nums[i];
        let mut end = start;
        while i + 1 < nums.len() && nums[i + 1] == end + 1 {
            i += 1;
            end = nums[i];
        }
        if start == end {
            parts.push(start.to_string());
        } else {
            parts.push(format!("{start}-{end}"));
        }
        i += 1;
    }
    parts.join(",")
}

fn wrap_commas(value: &str, group_size: usize) -> String {
    let parts: Vec<_> = value.split(',').map(str::trim).collect();
    let mut html = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            html.push_str(", ");
            if i % group_size == 0 {
                html.push_str("<br>");
            } else {
                html.push_str("<wbr>");
            }
        }
        html.push_str(&escape(part));
    }
    html
}

fn escape_attr(value: &str) -> String {
    escape(value).replace('\'', "&#39;")
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
        assert!(h.contains("R1, <wbr>R2"));
        assert!(h.contains("C123.html"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(!h.contains("<script>"));
    }

    #[test]
    fn long_designator_runs_are_compacted() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bom.csv");
        std::fs::write(
            &p,
            "Designator,Qty\n\"D1,D2,D3,D4,D5,D6,D7,D8,D9,D10\",10\n",
        )
        .unwrap();
        let h = table(&p).unwrap();
        assert!(h.contains("D(1-10)"));
        assert!(h.contains("title='D1,D2,D3,D4,D5,D6,D7,D8,D9,D10'"));
    }

    #[test]
    fn mixed_designators_get_break_opportunities() {
        let h = cell("D1,D3,R7,C1,J2,TP10", "References");
        assert!(h.contains("<br>"));
        assert!(h.contains("<wbr>"));
        assert!(h.contains("TP10"));
    }
}
