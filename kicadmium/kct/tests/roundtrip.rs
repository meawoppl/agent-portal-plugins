//! Parse -> compact -> parse must be structurally identical on real files.

use std::path::Path;

fn check(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    let tree = kct::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let again = kct::parse(&tree.to_kicad_string()).unwrap();
    assert_eq!(tree, again, "{}", path.display());
    let compact = kct::parse(&tree.to_compact_string()).unwrap();
    assert_eq!(tree, compact, "{}", path.display());
}

#[test]
fn fixtures_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut n = 0;
    let mut stack = vec![dir];
    let mut files = Vec::new();
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for path in files {
        if path
            .extension()
            .is_some_and(|e| e == "kicad_pcb" || e == "kicad_sch" || e == "kicad_sym" || e == "kicad_mod")
        {
            check(&path);
            n += 1;
        }
    }
    assert!(n > 0);
}
