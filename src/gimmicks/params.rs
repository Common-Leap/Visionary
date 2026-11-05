//! Typed personal-parameter edits (slices F0.1–F0.2).
//!
//! A personal file like `fighter/cloud/param/vl.prc` holds nested groups and
//! indexed lists, so one sparse edit is a [`NestedPath`](super::paths::NestedPath)
//! plus a [`ParamValue`]: the path names the leaf, the value carries the prc
//! type it must be written back as. The path serializes as its dotted string
//! spelling (see [`super::paths`]); the value reuses the existing model rather
//! than adding a parallel enum.
//!
//! Storage reuses [`ParamMod`]: the outer key is the game-relative personal
//! path from [`personal_path_for`], the inner keys are nested-path spellings.
//! That is the same sparse shape the shared `fighter_param.prc` traits editor
//! uses, so a project holds only authored leaves, never a whole-file copy.
//!
//! Slice F0.2 walks a real `vl.prc`: [`open_personal`] reads the file,
//! [`apply_edit`] writes one [`PersonalEdit`] into a cloned tree while
//! preserving the leaf's original prc type, and [`apply_personal_edits`]
//! applies the stored spelling-to-value map the way
//! [`FighterTraits::apply`](crate::roster::traits::FighterTraits::apply) does —
//! failures are collected as [`ApplyFailure`], never silently skipped. Missing
//! paths, bad indices, and type mismatches are errors naming fighter, path,
//! expected type, and actual value. [`read_leaf`] reports a leaf's current
//! typed value for later display and diffing. Persistence and migration stay
//! F0.3; export stays F0.5 — none of that lives here.
//!
//! Coercion and type names are reused from the traits owner
//! (`crate::roster::traits`), so both editors coerce identically; this module
//! only adds the nested walk and the fighter/path context.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use prc::ParamKind;
use serde::{Deserialize, Serialize};

use super::paths::{field_hash, is_raw_hash, NestedPath, Segment};
use crate::mod_project::{ParamMod, ParamValue};
use crate::roster::traits::{coerce, kind_name, to_param_value};

/// One authored leaf inside a fighter's personal prc.
///
/// The path is validated at construction and at deserialization: an invalid
/// spelling is an error naming its segment, never a string that breaks later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonalEdit {
    pub path: NestedPath,
    pub value: ParamValue,
}

impl PersonalEdit {
    /// Build an edit from an already-parsed path.
    pub fn from_path(path: NestedPath, value: ParamValue) -> Self {
        Self { path, value }
    }

    /// The stable spelling: the dotted path string.
    pub fn spelling(&self) -> String {
        self.path.to_string()
    }
}

/// Game-relative key for a fighter's personal prc inside [`ParamMod::files`].
///
/// Mirrors the `fighter/common/param/fighter_param.prc` convention the traits
/// editor owns: one game-relative path per file, so personal edits stay sparse
/// and per-fighter instead of copying a shared file.
pub fn personal_path_for(fighter: &str) -> anyhow::Result<String> {
    if fighter.is_empty() {
        anyhow::bail!("fighter name is empty; cannot key a personal prc");
    }
    if fighter.contains('/') || fighter.contains('\\') || fighter.contains('.') {
        anyhow::bail!("fighter {fighter:?} is not a plain fighter directory name");
    }
    Ok(format!("fighter/{fighter}/param/vl.prc"))
}

/// Locate one fighter's personal file across the data root and enabled mod
/// roots. Mirrors `FighterTraits::locate`: the first root that carries the
/// file wins, so an enabled mod's personal file shadows the base game's.
pub fn locate_personal(roots: &[PathBuf], fighter: &str) -> Option<PathBuf> {
    let rel = personal_path_for(fighter).ok()?;
    roots
        .iter()
        .map(|root| root.join(&rel))
        .find(|path| path.is_file())
}

/// Read a fighter's personal sparse edits out of a project.
///
/// Returns an empty map when the fighter has no personal key — a project saved
/// before personal edits existed simply has none, and loads unchanged.
pub fn personal_edits<'a>(params: &'a ParamMod, fighter: &str) -> &'a BTreeMap<String, ParamValue> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, ParamValue>> = std::sync::OnceLock::new();
    let empty = || EMPTY.get_or_init(BTreeMap::new);
    match personal_path_for(fighter) {
        Ok(key) => params.files.get(&key).unwrap_or_else(empty),
        Err(_) => empty(),
    }
}

/// Record one personal sparse edit, or clear it when the value returns to the
/// base file's.
///
/// Clearing matters for the same reason it does in the traits editor: an
/// override that agrees with the base pins a value that would otherwise track
/// the game and any mod underneath it. The error names fighter, path, and the
/// values involved; nothing is silently skipped.
pub fn record_personal_edit(
    params: &mut ParamMod,
    fighter: &str,
    path: &NestedPath,
    value: ParamValue,
    base: Option<ParamValue>,
) -> anyhow::Result<()> {
    let key = personal_path_for(fighter)?;
    if path.is_empty() {
        anyhow::bail!("fighter {fighter}: personal path is empty");
    }
    let spelling = path.to_string();
    let file = params.files.entry(key).or_default();
    if base == Some(value) {
        file.remove(&spelling);
    } else {
        file.insert(spelling, value);
    }
    let personal_key = personal_path_for(fighter)?;
    if file.is_empty() {
        params.files.remove(&personal_key);
    }
    Ok(())
}

/// True when every segment of the path is a resolved label.
///
/// Raw `0x…` segments keep an unknown field addressable (see [`super::paths`]),
/// but an unresolved authored label must not silently export — later slices
/// use this to refuse such edits at the UI (F0.4) and export (F0.5) gates.
pub fn path_is_resolved(path: &NestedPath) -> bool {
    !path.is_empty() && path.0.iter().all(|step| !is_raw_hash(&step.field))
}

/// Read a fighter's personal prc off disk.
///
/// The caller owns the tree from here: patch a clone and compare it against
/// the original, so untouched leaves are proven untouched rather than assumed.
pub fn open_personal(path: &Path, fighter: &str) -> anyhow::Result<prc::ParamStruct> {
    prc::open(path)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
        .with_context(|| format!("fighter {fighter}: reading {}", path.display()))
}

/// Write a patched personal tree back to disk, creating parent directories.
pub fn save_personal(root: &prc::ParamStruct, path: &Path, fighter: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    prc::save(path, root)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
        .with_context(|| format!("fighter {fighter}: writing {}", path.display()))
}

/// Apply one authored edit to a (cloned) personal tree, keeping the leaf's
/// original prc type.
///
/// Numbers coerce across widths the way the traits editor coerces; anything
/// that does not fit is refused rather than truncated. Every failure names
/// the fighter, the dotted spelling, and — for mismatches — the expected and
/// actual types.
pub fn apply_edit(
    root: &mut prc::ParamStruct,
    fighter: &str,
    edit: &PersonalEdit,
) -> anyhow::Result<()> {
    let spelling = edit.spelling();
    let slot = walk_mut(root, fighter, &spelling, &edit.path)?;
    let expected = kind_name(slot);
    let actual = value_kind_name(&edit.value);
    let replacement = coerce(slot, edit.value).map_err(|error| {
        anyhow::anyhow!(
            "fighter {fighter}: {spelling}: cannot write {actual} into {expected}: {error}"
        )
    })?;
    *slot = replacement;
    Ok(())
}

/// Report a leaf's current typed value without changing anything.
///
/// Non-scalar leaves (groups, lists, text) have no editable value; that is an
/// explicit error, not a silent default.
pub fn read_leaf(
    root: &prc::ParamStruct,
    fighter: &str,
    path: &NestedPath,
) -> anyhow::Result<ParamValue> {
    let spelling = path.to_string();
    let slot = walk_ref(root, fighter, &spelling, path)?;
    to_param_value(slot).ok_or_else(|| {
        anyhow::anyhow!(
            "fighter {fighter}: {spelling} is {}, which holds no editable value",
            kind_name(slot)
        )
    })
}

/// One sparse edit that did not apply, with its reason.
///
/// Returned by [`apply_personal_edits`] the way the traits editor returns
/// missing keys: an edit whose field is gone from the base file is one the
/// project still holds and the mod will not contain, and the export path has
/// to be able to name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFailure {
    pub path: String,
    pub reason: String,
}

/// Apply the stored spelling-to-value map for one fighter.
///
/// Every entry is attempted; failures — unparsable spellings, missing paths,
/// bad indices, type mismatches — are collected with reasons. The tree keeps
/// the edits that did apply, so the caller decides whether a partial patch is
/// acceptable; nothing is silently skipped either way.
pub fn apply_personal_edits(
    root: &mut prc::ParamStruct,
    fighter: &str,
    edits: &BTreeMap<String, ParamValue>,
) -> Vec<ApplyFailure> {
    let mut failures = Vec::new();
    for (spelling, value) in edits {
        let path = match NestedPath::parse(spelling) {
            Ok(path) => path,
            Err(error) => {
                failures.push(ApplyFailure {
                    path: spelling.clone(),
                    reason: error.to_string(),
                });
                continue;
            }
        };
        if let Err(error) = apply_edit(root, fighter, &PersonalEdit::from_path(path, *value)) {
            failures.push(ApplyFailure {
                path: spelling.clone(),
                reason: error.to_string(),
            });
        }
    }
    failures
}

/// Walk to the addressed slot for writing.
fn walk_mut<'a>(
    root: &'a mut prc::ParamStruct,
    fighter: &str,
    spelling: &str,
    path: &NestedPath,
) -> anyhow::Result<&'a mut ParamKind> {
    let [first, rest @ ..] = path.0.as_slice() else {
        anyhow::bail!("fighter {fighter}: personal path is empty");
    };
    let slot = struct_field_mut(root, fighter, spelling, first, 1)?;
    let target = index_slot_mut(slot, fighter, spelling, first, 1)?;
    descend_mut(target, fighter, spelling, first, 1, rest)
}

/// Walk to the addressed slot for reading; the immutable twin of [`walk_mut`].
fn walk_ref<'a>(
    root: &'a prc::ParamStruct,
    fighter: &str,
    spelling: &str,
    path: &NestedPath,
) -> anyhow::Result<&'a ParamKind> {
    let [first, rest @ ..] = path.0.as_slice() else {
        anyhow::bail!("fighter {fighter}: personal path is empty");
    };
    let slot = struct_field_ref(root, fighter, spelling, first, 1)?;
    let target = index_slot_ref(slot, fighter, spelling, first, 1)?;
    descend_ref(target, fighter, spelling, first, 1, rest)
}

fn descend_mut<'a>(
    target: &'a mut ParamKind,
    fighter: &str,
    spelling: &str,
    selected_by: &Segment,
    selected_position: usize,
    rest: &[Segment],
) -> anyhow::Result<&'a mut ParamKind> {
    let [segment, rest @ ..] = rest else {
        return Ok(target);
    };
    let group = match target {
        ParamKind::Struct(group) => group,
        ParamKind::List(items) => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {selected_position} names {:?}, which is a list of {}; an index is required to reach {:?}",
            selected_by.field,
            items.0.len(),
            segment.field,
        ),
        other => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {selected_position} names {:?}, which is {} and holds nothing deeper",
            selected_by.field,
            kind_name(other),
        ),
    };
    let slot = struct_field_mut(group, fighter, spelling, segment, selected_position + 1)?;
    let target = index_slot_mut(slot, fighter, spelling, segment, selected_position + 1)?;
    descend_mut(
        target,
        fighter,
        spelling,
        segment,
        selected_position + 1,
        rest,
    )
}

fn descend_ref<'a>(
    target: &'a ParamKind,
    fighter: &str,
    spelling: &str,
    selected_by: &Segment,
    selected_position: usize,
    rest: &[Segment],
) -> anyhow::Result<&'a ParamKind> {
    let [segment, rest @ ..] = rest else {
        return Ok(target);
    };
    let group = match target {
        ParamKind::Struct(group) => group,
        ParamKind::List(items) => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {selected_position} names {:?}, which is a list of {}; an index is required to reach {:?}",
            selected_by.field,
            items.0.len(),
            segment.field,
        ),
        other => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {selected_position} names {:?}, which is {} and holds nothing deeper",
            selected_by.field,
            kind_name(other),
        ),
    };
    let slot = struct_field_ref(group, fighter, spelling, segment, selected_position + 1)?;
    let target = index_slot_ref(slot, fighter, spelling, segment, selected_position + 1)?;
    descend_ref(
        target,
        fighter,
        spelling,
        segment,
        selected_position + 1,
        rest,
    )
}

/// Find a field of a struct by label or raw hash.
fn struct_field_mut<'a>(
    group: &'a mut prc::ParamStruct,
    fighter: &str,
    spelling: &str,
    segment: &Segment,
    position: usize,
) -> anyhow::Result<&'a mut ParamKind> {
    let wanted = usable_field_hash(fighter, spelling, segment, position)?;
    group
        .0
        .iter_mut()
        .find(|(hash, _)| hash.0 == wanted)
        .map(|(_, slot)| slot)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "fighter {fighter}: {spelling}: segment {position} names {:?}, which this file does not have",
                segment.field,
            )
        })
}

/// Immutable twin of [`struct_field_mut`].
fn struct_field_ref<'a>(
    group: &'a prc::ParamStruct,
    fighter: &str,
    spelling: &str,
    segment: &Segment,
    position: usize,
) -> anyhow::Result<&'a ParamKind> {
    let wanted = usable_field_hash(fighter, spelling, segment, position)?;
    group
        .0
        .iter()
        .find(|(hash, _)| hash.0 == wanted)
        .map(|(_, slot)| slot)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "fighter {fighter}: {spelling}: segment {position} names {:?}, which this file does not have",
                segment.field,
            )
        })
}

/// Step into a list element when the segment carries `[N]`; anything else
/// passes through untouched.
fn index_slot_mut<'a>(
    slot: &'a mut ParamKind,
    fighter: &str,
    spelling: &str,
    segment: &Segment,
    position: usize,
) -> anyhow::Result<&'a mut ParamKind> {
    let Some(wanted) = segment.index else {
        return Ok(slot);
    };
    match slot {
        ParamKind::List(items) => {
            let len = items.0.len();
            items.0.get_mut(wanted).ok_or_else(|| {
                anyhow::anyhow!(
                    "fighter {fighter}: {spelling}: segment {position} asks for {:?}[{wanted}], but the list holds only {len}",
                    segment.field,
                )
            })
        }
        other => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {position} asks for {:?}[{wanted}], but it is {}, not a list",
            segment.field,
            kind_name(other),
        ),
    }
}

/// Immutable twin of [`index_slot_mut`].
fn index_slot_ref<'a>(
    slot: &'a ParamKind,
    fighter: &str,
    spelling: &str,
    segment: &Segment,
    position: usize,
) -> anyhow::Result<&'a ParamKind> {
    let Some(wanted) = segment.index else {
        return Ok(slot);
    };
    match slot {
        ParamKind::List(items) => items.0.get(wanted).ok_or_else(|| {
            anyhow::anyhow!(
                "fighter {fighter}: {spelling}: segment {position} asks for {:?}[{wanted}], but the list holds only {}",
                segment.field,
                items.0.len(),
            )
        }),
        other => anyhow::bail!(
            "fighter {fighter}: {spelling}: segment {position} asks for {:?}[{wanted}], but it is {}, not a list",
            segment.field,
            kind_name(other),
        ),
    }
}

/// Resolve a segment's field to the hash the file stores: labels hash by name,
/// raw `0x…` hashes parse directly so unknown fields stay addressable.
fn usable_field_hash(
    fighter: &str,
    spelling: &str,
    segment: &Segment,
    position: usize,
) -> anyhow::Result<u64> {
    field_hash(&segment.field).ok_or_else(|| {
        anyhow::anyhow!(
            "fighter {fighter}: {spelling}: segment {position} names {:?}, which is not a usable field",
            segment.field,
        )
    })
}

/// The prc type name a [`ParamValue`] carries, for errors that name an
/// expected type and an actual one (the F0.2 mismatch reports).
pub fn value_kind_name(value: &ParamValue) -> &'static str {
    match value {
        ParamValue::Bool(_) => "bool",
        ParamValue::I8(_) => "i8",
        ParamValue::U8(_) => "u8",
        ParamValue::I16(_) => "i16",
        ParamValue::U16(_) => "u16",
        ParamValue::I32(_) => "i32",
        ParamValue::U32(_) => "u32",
        ParamValue::Float(_) => "float",
        ParamValue::Hash(_) => "hash",
    }
}

#[cfg(test)]
impl PersonalEdit {
    /// Build an edit from the dotted spelling, rejecting invalid paths.
    pub fn new(path: &str, value: ParamValue) -> anyhow::Result<Self> {
        Ok(Self {
            path: NestedPath::parse(path)?,
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOUD_GAUGE: &str = "param_special_lw[0].limit_gauge_add";

    fn cloud_edit() -> PersonalEdit {
        PersonalEdit::new(CLOUD_GAUGE, ParamValue::Float(0.667)).unwrap()
    }

    #[test]
    fn a_cloud_tunable_round_trips_through_model_and_json() {
        let edit = cloud_edit();
        assert_eq!(edit.spelling(), CLOUD_GAUGE);
        let json = serde_json::to_string(&edit).unwrap();
        assert!(
            json.contains(CLOUD_GAUGE),
            "the stable spelling travels in JSON: {json}"
        );
        let back: PersonalEdit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, edit);
    }

    #[test]
    fn invalid_paths_are_rejected_with_a_reason() {
        for raw in [
            "",
            "param_special_lw..limit_gauge_add",
            "param_special_lw[x].limit_gauge_add",
            "param_special_lw[].limit_gauge_add",
        ] {
            let error = PersonalEdit::new(raw, ParamValue::Float(1.0)).unwrap_err();
            assert!(!error.to_string().is_empty(), "{raw:?} needs a reason");
        }
        let error = serde_json::from_str::<PersonalEdit>(
            r#"{"path":"walk_speed_mul[","value":{"Float":1.0}}"#,
        )
        .unwrap_err();
        assert!(
            !error.to_string().is_empty(),
            "an invalid spelling fails at load"
        );
    }

    #[test]
    fn an_unresolved_path_is_flagged_but_stays_addressable() {
        let edit = PersonalEdit::new("param_special_lw[0].0x1234abcd", ParamValue::Float(1.0))
            .expect("raw hashes stay addressable");
        assert!(!path_is_resolved(&edit.path));
        assert!(path_is_resolved(&cloud_edit().path));
        assert!(!path_is_resolved(&NestedPath::default()));
    }
}
