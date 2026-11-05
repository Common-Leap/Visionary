//! Import diffs and patched personal-prc export (slice F0.5).
//!
//! Two directions around the same sparse map (`fighter/<name>/param/vl.prc`
//! → nested-path spelling → [`ParamValue`](crate::mod_project::ParamValue)):
//!
//! * [`export_personal`] patches a cloned base tree per fighter with
//!   [`apply_personal_edits`](super::params::apply_personal_edits) and writes
//!   it into the exported mod folder, mirroring how
//!   `crate::roster::export::export_params` rebuilds the shared
//!   `fighter_param.prc`. Only named leaves change: the tree is cloned,
//!   patched, and saved, so untouched leaves are equal by construction.
//!   Unresolved (`0x…`) spellings fail closed — skipped with a report warning
//!   naming fighter and path, never written as a guess — as do apply
//!   failures. Edits whose field is gone stay saved in the project, exactly
//!   like the traits export's missing-field warnings.
//! * [`diff_personal`] diffs a modded personal file against its base twin and
//!   reproduces the sparse map, mirroring
//!   `crate::mod_import::diff_fighter_param`. Changed scalar leaves are
//!   adopted under their label (hex fallback when a label is missing,
//!   honestly); added scalar leaves are adopted so the export can name them
//!   missing; removed leaves, changed text, and changed container types are
//!   reported as unknown, never fabricated.
//!
//! `src/mod_export.rs` needs no change for this slice: it generates
//! ACMD/effect source and serializes the whole project file, which already
//! carries the sparse personal edits from slice F0.3. The mod-folder prc
//! writing hooks in next to `export_params` at the
//! `RosterWindow::export_into` call site; the import diff hooks into
//! `import_mod_as_project` beside the `fighter_param` pass.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use prc::ParamKind;

use super::params::{
    apply_personal_edits, locate_personal, open_personal, path_is_resolved, personal_edits,
    personal_path_for, save_personal,
};
use super::paths::NestedPath;
use crate::mod_project::{ParamMod, ParamValue};
use crate::roster::export::RosterExport;
use crate::roster::traits::to_param_value;

/// True for a plain fighter's personal file: `fighter/<name>/param/vl.prc`.
///
/// The fighter segment must pass [`personal_path_for`], so traversal (`..`),
/// subdirectories, and wrong filenames never match. Import uses this to route
/// personal files to the diff pass; everything else `.prc` stays
/// reference-only.
pub fn is_personal_prc_path(game_path: &str) -> bool {
    let Some(rest) = game_path.strip_prefix("fighter/") else {
        return false;
    };
    let Some((fighter, tail)) = rest.split_once('/') else {
        return false;
    };
    tail == "param/vl.prc" && personal_path_for(fighter).is_ok()
}

/// Write every fighter's personal edits into patched `vl.prc` files.
///
/// One file per fighter holds one fighter, so each file is rebuilt from its
/// own base clone — unlike the shared `fighter_param.prc`, fighters can never
/// overwrite each other's changes here.
///
/// A fighter with no readable base file, or whose every edit is blocked, gets
/// no file and a warning naming what is missing. Blocked edits stay saved in
/// the project; the warning is what keeps that honest instead of silent.
pub fn export_personal(
    mod_root: &Path,
    roots: &[PathBuf],
    params: &BTreeMap<String, ParamMod>,
    report: &mut RosterExport,
) -> anyhow::Result<()> {
    let with_edits: Vec<(&String, &BTreeMap<String, ParamValue>)> = params
        .iter()
        .map(|(fighter, param_mod)| (fighter, personal_edits(param_mod, fighter)))
        .filter(|(_, edits)| !edits.is_empty())
        .collect();
    if with_edits.is_empty() {
        return Ok(());
    }

    for (fighter, edits) in with_edits {
        let rel = match personal_path_for(fighter) {
            Ok(rel) => rel,
            Err(error) => {
                report.warnings.push(format!(
                    "{fighter}: personal edits were not written — {error:#}. \
                     They are still saved in the project."
                ));
                continue;
            }
        };
        let Some(source) = locate_personal(roots, fighter) else {
            report.warnings.push(format!(
                "{fighter}: {} personal edit(s) were not written: {rel} was not found. Dump the \
                 fighter's folder along with the rest of fighter/. They are still saved in the project.",
                edits.len()
            ));
            continue;
        };
        let base = match open_personal(&source, fighter) {
            Ok(tree) => tree,
            Err(error) => {
                report.warnings.push(format!(
                    "{fighter}: personal edits were not written — {error:#}. \
                     They are still saved in the project."
                ));
                continue;
            }
        };

        // The export gate: an unresolved authored label cannot silently
        // export, so raw-`0x…` spellings and unparsable spellings are refused
        // here with a reason, before the walker ever sees them.
        let mut resolved: BTreeMap<String, ParamValue> = BTreeMap::new();
        for (spelling, value) in edits {
            match NestedPath::parse(spelling) {
                Ok(path) if path_is_resolved(&path) => {
                    resolved.insert(spelling.clone(), *value);
                }
                Ok(_) => report.warnings.push(format!(
                    "{fighter}: personal edit {spelling:?} was not written: it uses an \
                     unresolved label, which cannot be exported. Resolve the label against \
                     ParamLabels.csv and save the edit again. It is still saved in the project."
                )),
                Err(error) => report.warnings.push(format!(
                    "{fighter}: personal edit {spelling:?} was not written — {error:#}. \
                     It is still saved in the project."
                )),
            }
        }

        let mut patched = base.clone();
        let failures = apply_personal_edits(&mut patched, fighter, &resolved);
        for failure in &failures {
            report.warnings.push(format!(
                "{fighter}: personal edit {:?} was not written — {}. \
                 It is still saved in the project.",
                failure.path, failure.reason
            ));
        }
        let applied = resolved.len().saturating_sub(failures.len());
        if applied == 0 {
            report.warnings.push(format!(
                "{fighter}: none of {} personal edit(s) could be written, so no {rel} was \
                 emitted. They are still saved in the project.",
                edits.len()
            ));
            continue;
        }
        let destination = mod_root.join(&rel);
        save_personal(&patched, &destination, fighter)?;
        report.files.push(rel);
    }
    Ok(())
}

/// Diff a modded personal file against its base twin.
///
/// Returns the sparse spelling-to-value map plus unknown notes. Changed and
/// added scalar leaves are adopted; removed leaves, changed text, and changed
/// container types are reported, never fabricated. Re-diffing an exported
/// patch against its base reproduces the exported map — that round trip is
/// the acceptance test.
pub fn diff_personal(
    base: &prc::ParamStruct,
    modded: &prc::ParamStruct,
    fighter: &str,
    labels: &HashMap<u64, String>,
) -> (BTreeMap<String, ParamValue>, Vec<String>) {
    let mut edits = BTreeMap::new();
    let mut unknown = Vec::new();
    diff_fields(
        Some(&base.0),
        &modded.0,
        fighter,
        "",
        labels,
        &mut edits,
        &mut unknown,
    );
    unknown.sort();
    unknown.dedup();
    (edits, unknown)
}

/// Label or honest hex, the same fallback the `fighter_param` diff uses: a
/// field whose hash has no label keeps its hex name on import, and the export
/// gate refuses to write it until the label resolves.
fn field_name(hash: u64, labels: &HashMap<u64, String>) -> String {
    labels
        .get(&hash)
        .cloned()
        .unwrap_or_else(|| format!("{hash:#x}"))
}

fn diff_fields(
    base: Option<&[(prc::hash40::Hash40, ParamKind)]>,
    modded: &[(prc::hash40::Hash40, ParamKind)],
    fighter: &str,
    prefix: &str,
    labels: &HashMap<u64, String>,
    edits: &mut BTreeMap<String, ParamValue>,
    unknown: &mut Vec<String>,
) {
    let base_by_hash: HashMap<u64, &ParamKind> = base
        .unwrap_or_default()
        .iter()
        .map(|(hash, value)| (hash.0, value))
        .collect();
    for (hash, mod_value) in modded {
        let name = field_name(hash.0, labels);
        let spelling = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}.{name}")
        };
        diff_slot(
            base_by_hash.get(&hash.0).copied(),
            mod_value,
            fighter,
            &spelling,
            labels,
            edits,
            unknown,
        );
    }
    if let Some(base_fields) = base {
        for (hash, _) in base_fields {
            if !modded.iter().any(|(other, _)| other.0 == hash.0) {
                let name = field_name(hash.0, labels);
                let spelling = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}.{name}")
                };
                unknown.push(format!(
                    "{fighter}: {spelling} is in the base file but absent from the mod file; not adopted"
                ));
            }
        }
    }
}

fn diff_slot(
    base: Option<&ParamKind>,
    modded: &ParamKind,
    fighter: &str,
    spelling: &str,
    labels: &HashMap<u64, String>,
    edits: &mut BTreeMap<String, ParamValue>,
    unknown: &mut Vec<String>,
) {
    // Equal subtrees — including equal groups, lists, and text — hold no edit.
    // This is also what keeps identical files diffing to nothing.
    if base == Some(modded) {
        return;
    }
    match (base, modded) {
        (Some(ParamKind::Struct(base_group)), ParamKind::Struct(mod_group)) => {
            diff_fields(
                Some(&base_group.0),
                &mod_group.0,
                fighter,
                spelling,
                labels,
                edits,
                unknown,
            );
        }
        (None, ParamKind::Struct(mod_group)) => {
            diff_fields(
                None,
                &mod_group.0,
                fighter,
                spelling,
                labels,
                edits,
                unknown,
            );
        }
        (Some(ParamKind::List(base_items)), ParamKind::List(mod_items)) => {
            diff_list(
                Some(&base_items.0),
                &mod_items.0,
                fighter,
                spelling,
                labels,
                edits,
                unknown,
            );
        }
        (None, ParamKind::List(mod_items)) => {
            diff_list(
                None,
                &mod_items.0,
                fighter,
                spelling,
                labels,
                edits,
                unknown,
            );
        }
        _ => match (base, to_param_value(modded)) {
            // A changed scalar leaf, or a scalar leaf the base file lacks: a
            // real mod value is still adopted under its label, so the export
            // can name what is missing rather than guessing. The base side
            // must hold a scalar too — a spelling addressing a group or list
            // names no leaf, so a container-to-scalar change is reported
            // below rather than adopted as a value the walker could never
            // write back.
            (Some(leaf), Some(mod_value))
                if to_param_value(leaf).is_some() && to_param_value(leaf) != Some(mod_value) =>
            {
                edits.insert(spelling.to_string(), mod_value);
            }
            (None, Some(mod_value)) => {
                edits.insert(spelling.to_string(), mod_value);
            }
            // Text and changed container types have no sparse form; report.
            _ => unknown.push(format!(
                "{fighter}: {spelling} changed type or holds text, which a sparse edit \
                 cannot hold; not adopted"
            )),
        },
    }
}

fn diff_list(
    base: Option<&[ParamKind]>,
    modded: &[ParamKind],
    fighter: &str,
    spelling: &str,
    labels: &HashMap<u64, String>,
    edits: &mut BTreeMap<String, ParamValue>,
    unknown: &mut Vec<String>,
) {
    let base_items = base.unwrap_or_default();
    for (index, mod_item) in modded.iter().enumerate() {
        diff_slot(
            base_items.get(index),
            mod_item,
            fighter,
            &format!("{spelling}[{index}]"),
            labels,
            edits,
            unknown,
        );
    }
    if base_items.len() > modded.len() {
        for index in modded.len()..base_items.len() {
            unknown.push(format!(
                "{fighter}: {spelling}[{index}] is in the base file but absent from the mod file; not adopted"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{cloud_prc, good_dump};
    use super::*;
    use crate::gimmicks::params::{open_personal, record_personal_edit};
    use crate::gimmicks::paths::NestedPath;

    fn labels() -> HashMap<u64, String> {
        // Labels for every hash the fixture uses, so the happy-path diff is
        // fully resolved; tests that need a missing label remove entries.
        let mut labels = HashMap::new();
        for name in [
            "param_special_lw",
            "limit_gauge_add",
            "limit_gauge_take",
            "charge_per_frame",
            "walk_speed_mul",
            "limit_max_frames",
        ] {
            labels.insert(hash40::hash40(name).0, name.to_string());
        }
        labels
    }

    fn cloud_params(pairs: &[(&str, ParamValue)]) -> BTreeMap<String, ParamMod> {
        let mut params = ParamMod::default();
        for (spelling, value) in pairs {
            let path = NestedPath::parse(spelling).unwrap();
            record_personal_edit(&mut params, "cloud", &path, *value, None).unwrap();
        }
        BTreeMap::from([("cloud".to_string(), params)])
    }

    fn personal_rel() -> String {
        "fighter/cloud/param/vl.prc".to_string()
    }

    #[test]
    fn only_plain_fighter_personal_files_match() {
        assert!(is_personal_prc_path("fighter/cloud/param/vl.prc"));
        assert!(!is_personal_prc_path(
            "fighter/common/param/fighter_param.prc"
        ));
        assert!(!is_personal_prc_path("fighter/cloud/param/other.prc"));
        assert!(!is_personal_prc_path("fighter/cloud/param/vl.prc.bak"));
        assert!(!is_personal_prc_path("fighter/../cloud/param/vl.prc"));
        assert!(!is_personal_prc_path("fighter//param/vl.prc"));
        assert!(!is_personal_prc_path("fighter/cloud/param/vl.prc/extra"));
        assert!(!is_personal_prc_path("FIGHTER/cloud/param/vl.prc"));
    }

    #[test]
    fn export_writes_only_named_leaves() {
        let dump = good_dump();
        let out = tempfile::tempdir().unwrap();
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[dump.path().to_path_buf()],
            &cloud_params(&[
                (
                    "param_special_lw[0].limit_gauge_add",
                    ParamValue::Float(0.9),
                ),
                ("walk_speed_mul", ParamValue::Float(1.0)),
            ]),
            &mut report,
        )
        .unwrap();

        assert_eq!(report.files, vec![personal_rel()]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let patched = open_personal(&out.path().join(personal_rel()), "cloud").unwrap();
        // The addressed leaves changed…
        let mut expected = cloud_prc();
        let ParamKind::List(rows) = &mut expected.0[0].1 else {
            panic!("the fixture changed shape");
        };
        let ParamKind::Struct(first) = &mut rows.0[0] else {
            panic!("the fixture changed shape");
        };
        first.0[0].1 = ParamKind::Float(0.9);
        expected.0[1].1 = ParamKind::Float(1.0);
        // …and the whole tree differs from the base in exactly those leaves:
        // rebuilt by hand rather than trusting the exporter.
        assert_eq!(patched, expected);
    }

    #[test]
    fn unresolved_spellings_fail_closed_but_resolved_edits_still_write() {
        let dump = good_dump();
        let out = tempfile::tempdir().unwrap();
        let raw = format!(
            "param_special_lw[0].0x{:x}",
            hash40::hash40("limit_gauge_take").0
        );
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[dump.path().to_path_buf()],
            &cloud_params(&[
                ("walk_speed_mul", ParamValue::Float(1.0)),
                (raw.as_str(), ParamValue::Float(2.0)),
            ]),
            &mut report,
        )
        .unwrap();

        // The resolved edit wrote; the raw-hash edit did not touch its leaf.
        assert_eq!(report.files, vec![personal_rel()]);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].contains("cloud")
                && report.warnings[0].contains(&raw)
                && report.warnings[0].contains("unresolved"),
            "{}",
            report.warnings[0]
        );
        let patched = open_personal(&out.path().join(personal_rel()), "cloud").unwrap();
        let mut expected = cloud_prc();
        expected.0[1].1 = ParamKind::Float(1.0);
        assert_eq!(patched, expected);
    }

    #[test]
    fn a_fighter_with_only_blocked_edits_emits_no_file() {
        let dump = good_dump();
        let out = tempfile::tempdir().unwrap();
        let raw = format!("0x{:x}", hash40::hash40("walk_speed_mul").0);
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[dump.path().to_path_buf()],
            &cloud_params(&[
                (raw.as_str(), ParamValue::Float(2.0)),
                ("no_such_leaf", ParamValue::Float(1.0)),
            ]),
            &mut report,
        )
        .unwrap();

        assert!(report.files.is_empty(), "{:?}", report.files);
        assert!(
            !out.path().join(personal_rel()).exists(),
            "a fully blocked fighter emits no file"
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("none of 2")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_missing_base_is_reported_not_written() {
        let out = tempfile::tempdir().unwrap();
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[],
            &cloud_params(&[("walk_speed_mul", ParamValue::Float(1.0))]),
            &mut report,
        )
        .unwrap();
        assert!(report.files.is_empty());
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].contains("cloud") && report.warnings[0].contains(&personal_rel()),
            "{}",
            report.warnings[0]
        );

        // No edits at all is a quiet no-op, so an untouched project exports
        // exactly what it did before this slice.
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[],
            &BTreeMap::from([("cloud".to_string(), ParamMod::default())]),
            &mut report,
        )
        .unwrap();
        assert!(report.files.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn apply_failures_are_named_and_keep_the_good_edits() {
        let dump = good_dump();
        let out = tempfile::tempdir().unwrap();
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[dump.path().to_path_buf()],
            &cloud_params(&[
                ("walk_speed_mul", ParamValue::Float(1.0)),
                ("param_special_lw[0].no_such_leaf", ParamValue::Float(1.0)),
            ]),
            &mut report,
        )
        .unwrap();

        assert_eq!(report.files, vec![personal_rel()]);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].contains("cloud")
                && report.warnings[0].contains("param_special_lw[0].no_such_leaf"),
            "{}",
            report.warnings[0]
        );
        let patched = open_personal(&out.path().join(personal_rel()), "cloud").unwrap();
        let mut expected = cloud_prc();
        expected.0[1].1 = ParamKind::Float(1.0);
        assert_eq!(patched, expected);
    }

    #[test]
    fn diff_finds_only_changed_scalar_leaves() {
        let mut modded = cloud_prc();
        let ParamKind::List(rows) = &mut modded.0[0].1 else {
            panic!("the fixture changed shape");
        };
        let ParamKind::Struct(first) = &mut rows.0[0] else {
            panic!("the fixture changed shape");
        };
        first.0[0].1 = ParamKind::Float(0.9);
        modded.0[2].1 = ParamKind::I32(600);

        let (edits, unknown) = diff_personal(&cloud_prc(), &modded, "cloud", &labels());
        assert_eq!(
            edits,
            BTreeMap::from([
                (
                    "param_special_lw[0].limit_gauge_add".to_string(),
                    ParamValue::Float(0.9)
                ),
                ("limit_max_frames".to_string(), ParamValue::I32(600)),
            ])
        );
        assert!(unknown.is_empty(), "{unknown:?}");
    }

    #[test]
    fn diff_reports_what_a_sparse_edit_cannot_hold() {
        use prc::hash40::Hash40;
        let base = cloud_prc();
        let hash = |name: &str| Hash40(hash40::hash40(name).0);
        let mut modded = base.clone();
        // Added scalar leaf: adopted, so the export names it missing.
        modded.0.push((hash("fresh_scalar"), ParamKind::Float(3.0)));
        // Removed leaf, changed text, changed container type: reported.
        modded.0.retain(|(h, _)| *h != hash("walk_speed_mul"));
        modded
            .0
            .push((hash("fresh_text"), ParamKind::Str("hi".to_string())));
        let ParamKind::List(rows) = &mut modded.0[0].1 else {
            panic!("the fixture changed shape");
        };
        rows.0[0] = ParamKind::Float(9.0);

        let mut labels = labels();
        labels.insert(hash40::hash40("fresh_scalar").0, "fresh_scalar".to_string());
        labels.insert(hash40::hash40("fresh_text").0, "fresh_text".to_string());
        let (edits, unknown) = diff_personal(&base, &modded, "cloud", &labels);
        assert_eq!(
            edits,
            BTreeMap::from([("fresh_scalar".to_string(), ParamValue::Float(3.0))])
        );
        assert_eq!(unknown.len(), 3, "{unknown:?}");
        assert!(
            unknown.iter().any(|u| u.contains("walk_speed_mul")),
            "{unknown:?}"
        );
        assert!(
            unknown.iter().any(|u| u.contains("fresh_text")),
            "{unknown:?}"
        );
        assert!(
            unknown.iter().any(|u| u.contains("param_special_lw[0]")),
            "{unknown:?}"
        );
    }

    #[test]
    fn diff_uses_hex_for_hashes_without_labels() {
        let mut modded = cloud_prc();
        modded.0[1].1 = ParamKind::Float(2.0);
        let (edits, unknown) = diff_personal(&cloud_prc(), &modded, "cloud", &HashMap::new());
        let hex = format!("0x{:x}", hash40::hash40("walk_speed_mul").0);
        assert_eq!(
            edits,
            BTreeMap::from([(hex.clone(), ParamValue::Float(2.0))]),
            "a label-less change stays honest hex, for the export gate to refuse"
        );
        assert!(unknown.is_empty(), "{unknown:?}");
    }

    #[test]
    fn export_then_diff_reproduces_the_sparse_edit() {
        let dump = good_dump();
        let out = tempfile::tempdir().unwrap();
        let sparse = BTreeMap::from([
            (
                "param_special_lw[0].limit_gauge_add".to_string(),
                ParamValue::Float(0.9),
            ),
            ("limit_max_frames".to_string(), ParamValue::I32(600)),
        ]);
        let mut params = ParamMod::default();
        for (spelling, value) in &sparse {
            let path = NestedPath::parse(spelling).unwrap();
            record_personal_edit(&mut params, "cloud", &path, *value, None).unwrap();
        }
        let mut report = RosterExport::default();
        export_personal(
            out.path(),
            &[dump.path().to_path_buf()],
            &BTreeMap::from([("cloud".to_string(), params)]),
            &mut report,
        )
        .unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let base = open_personal(&dump.path().join(personal_rel()), "cloud").unwrap();
        let patched = open_personal(&out.path().join(personal_rel()), "cloud").unwrap();
        let (edits, unknown) = diff_personal(&base, &patched, "cloud", &labels());
        assert_eq!(edits, sparse, "re-import reproduces the sparse edit");
        assert!(unknown.is_empty(), "{unknown:?}");
    }
}
