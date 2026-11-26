//! A survey of real moveset projects: what the source index finds in each, and which lines the
//! parsers leave untyped. Run with `VISIONARY_MOVESETS=<dir holding moveset repos>`:
//!
//! ```text
//! cargo test moveset_survey -- --ignored --nocapture
//! ```

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

use crate::data::{AcmdStmt, EffectMacro, EffectStmt, ExcuteStmt};

fn raw_key(line: &str) -> String {
    let line = line.trim();
    let end = line
        .find(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '!' || c == '.')
        })
        .unwrap_or(line.len());
    let key = &line[..end];
    if key.is_empty() {
        line.chars().take(24).collect()
    } else {
        key.to_string()
    }
}

fn walk_acmd(stmts: &[AcmdStmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            AcmdStmt::Raw(line) => out.push(line.clone()),
            AcmdStmt::Excute(inner) => {
                for s in inner {
                    if let ExcuteStmt::Raw(line) = s {
                        out.push(line.clone());
                    }
                }
            }
            AcmdStmt::Loop { body, .. } | AcmdStmt::RawBlock { body, .. } => walk_acmd(body, out),
            _ => {}
        }
    }
}

fn walk_effect(stmts: &[EffectStmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            EffectStmt::Raw(line) => out.push(line.clone()),
            EffectStmt::Excute(inner) => {
                for m in inner {
                    if let EffectMacro::Raw(line) = m {
                        out.push(line.clone());
                    }
                }
            }
            EffectStmt::Loop { body, .. } | EffectStmt::Cond { body, .. } => walk_effect(body, out),
            _ => {}
        }
    }
}

#[test]
#[ignore]
fn moveset_survey() {
    let Some(root) = std::env::var_os("VISIONARY_MOVESETS") else {
        eprintln!("skipped: set VISIONARY_MOVESETS");
        return;
    };
    let root = Path::new(&root);
    let mut raw_counts: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(root).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(only) = std::env::var_os("VISIONARY_SURVEY_ONLY") {
            if name != only.to_string_lossy() {
                continue;
            }
        }
        let index = match crate::acmd_src::SourceIndex::build(&dir) {
            Ok(index) => index,
            Err(e) => {
                println!("{name}: ERROR {e}");
                continue;
            }
        };
        let mut fighters: Vec<_> = index.fighters.keys().cloned().collect();
        fighters.sort();
        let mut typed = 0usize;
        let mut raw_total = 0usize;
        for fighter in &index.fighters {
            for (script, site) in &fighter.1.scripts {
                let body = match &site.delegate {
                    Some(d) => d.body.clone(),
                    None => {
                        let Ok(text) = std::fs::read_to_string(&site.file) else {
                            continue;
                        };
                        let Some(src) = text.get(site.span.clone()) else {
                            continue;
                        };
                        src.to_string()
                    }
                };
                let body = index.hash_macros.expand_in(&body).into_owned();
                let body = index.helpers.inline(&body).into_owned();
                let body = index.consts.substitute(&body).into_owned();
                // Give the parsers the header they look for.
                let body = match body.find("fn ") {
                    Some(at) => {
                        let rest = &body[at + 3..];
                        let end = rest
                            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                            .unwrap_or(0);
                        format!("{}fn {script}{}", &body[..at], &rest[end..])
                    }
                    None => body,
                };
                let mut raws = Vec::new();
                if script.starts_with("game_") {
                    walk_acmd(&crate::acmd::parse_acmd_script(&body).stmts, &mut raws);
                } else if script.starts_with("effect_") {
                    walk_effect(&crate::acmd::parse_effect_script(&body).stmts, &mut raws);
                } else if script.starts_with("sound_") {
                    walk_acmd(&crate::acmd::parse_sound_script(&body).stmts, &mut raws);
                } else if script.starts_with("expression_") {
                    walk_acmd(
                        &crate::acmd::parse_expression_script(&body).stmts,
                        &mut raws,
                    );
                }
                typed += 1;
                raw_total += raws.len();
                for line in raws {
                    let category = script.split('_').next().unwrap_or("");
                    let key = format!("{category}: {}", raw_key(&line));
                    let slot = raw_counts.entry(key).or_default();
                    slot.0 += 1;
                    if slot.1.len() < 3 {
                        let mut sample: String = line.trim().chars().take(160).collect();
                        sample = format!("[{name}] {sample}");
                        slot.1.push(sample);
                    }
                }
            }
        }
        let notes: usize = index
            .fighters
            .values()
            .flat_map(|f| f.scripts.values())
            .filter(|s| s.delegate.is_some())
            .count();
        println!(
            "{name}: files={} fighters={:?} scripts={} conflicts={} delegates={} raw_lines={}",
            index.files.len(),
            fighters,
            typed,
            index.conflict_count(),
            notes,
            raw_total
        );
        if std::env::var_os("VISIONARY_SURVEY_PLAN").is_some() {
            let sources: Vec<String> = index
                .files
                .iter()
                .filter_map(|f| std::fs::read_to_string(f).ok())
                .collect();
            let sd = crate::scratch_dirs::emulator_sd_root();
            match crate::moveset_build::plan(&index.files, &sources, sd.as_deref()) {
                Ok(plan) => println!(
                    "    PLAN crate={} nro={} found={:?}",
                    plan.crate_dir.display(),
                    plan.nro_name,
                    plan.found
                ),
                Err(e) => println!("    PLAN error: {e}"),
            }
        }
        for conflict in index.conflicts.iter().take(3) {
            println!("    CONFLICT {}", conflict.description());
        }
    }
    let mut sorted: Vec<_> = raw_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("\n== untyped lines by leading call ==");
    for (key, (count, samples)) in sorted.iter().take(150) {
        println!("{count:6}  {key}");
        for s in samples {
            println!("          {s}");
        }
    }
}
