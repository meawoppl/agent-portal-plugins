//! Hierarchical net-name normalization and matching (port of
//! `kicad_tools.router.net_names`, Issue #4149).

/// Sheet-local suffix after the last `/`.
pub fn net_name_suffix(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// `suffix -> [board net names]`, insertion ordered.
pub fn build_net_name_index(board: &[String]) -> Vec<(String, Vec<String>)> {
    let mut index: Vec<(String, Vec<String>)> = Vec::new();
    for raw in board {
        let suffix = net_name_suffix(raw).to_string();
        let i = match index.iter().position(|(s, _)| *s == suffix) {
            Some(i) => i,
            None => {
                index.push((suffix, Vec::new()));
                index.len() - 1
            }
        };
        if !index[i].1.contains(raw) {
            index[i].1.push(raw.clone());
        }
    }
    index
}

/// Resolution of one user key: `Ok(Some(board))` matched, `Err(candidates)`
/// ambiguous, `Ok(None)` unmatched.
pub fn resolve_net_key(
    user_key: &str,
    index: &[(String, Vec<String>)],
) -> Result<Option<String>, Vec<String>> {
    if user_key.contains('/') && index.iter().any(|(_, b)| b.iter().any(|r| r == user_key)) {
        return Ok(Some(user_key.to_string()));
    }
    let suffix = net_name_suffix(user_key);
    let cands = index
        .iter()
        .find(|(s, _)| s == suffix)
        .map(|(_, b)| b.clone())
        .unwrap_or_default();
    match cands.len() {
        0 => Ok(None),
        1 => Ok(Some(cands[0].clone())),
        _ => Err(cands),
    }
}

/// `resolve_net_class_map_keys(...).resolved` as insertion-ordered
/// `(board_net, user_key)` pairs (a later key for the same board net
/// replaces the earlier value in place, like a dict assignment).
pub fn resolve_net_class_map_keys(user_keys: &[String], board: &[String]) -> Vec<(String, String)> {
    let index = build_net_name_index(board);
    let mut resolved: Vec<(String, String)> = Vec::new();
    for key in user_keys {
        if let Ok(Some(board_net)) = resolve_net_key(key, &index) {
            match resolved.iter_mut().find(|(b, _)| *b == board_net) {
                Some(e) => e.1 = key.clone(),
                None => resolved.push((board_net, key.clone())),
            }
        }
    }
    resolved
}
