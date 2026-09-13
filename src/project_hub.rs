//! Project Hub: one current project, its path, and the cold-start choices.
//!
//! Visionary edits used to live in memory until manually exported: launches started
//! blank, and there was no notion of "the project I have open". This module gives
//! that notion a shape that can be tested without a GUI:
//!
//! * [`CurrentProject`] tracks the open project's path (persisted) and whether it
//!   holds unsaved edits. Save writes silently when a path is known; Save As
//!   relocates; Export/Load adopt the path they touched.
//! * [`HubState`] models the cold-start hub — Resume last / New / Open
//!   (`modproject.json`) / Import mod / Recent / Browse without project — and the
//!   mid-session reopen with its unsaved-edits guard.
//! * Persistence helpers take an explicit config directory so tests run against a
//!   tempdir instead of the user's real config.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use ssbh_data::anim_data::{AnimData, GroupType};
use ssbh_data::matl_data::MatlData;
use ssbh_data::modl_data::ModlData;
use ssbh_data::prelude::{SkelData, SsbhData};

#[cfg(test)]
#[path = "../plugins/slight_replica/src/slight/effect_viewer/asset_path.rs"]
mod carrier_paths;

/// How many recent projects are remembered.
pub const MAX_RECENT_PROJECTS: usize = 10;

/// Config file holding the last open project path (one absolute path, or empty).
pub const LAST_PROJECT_KEY: &str = "project_path";
/// Config file holding recent project paths (one absolute path per line).
pub const RECENT_PROJECTS_KEY: &str = "recent_projects";

/// One current project holds every edit.
#[derive(Debug, Clone, Default)]
pub struct CurrentProject {
    /// Where Save writes silently. `None` means "never saved yet".
    pub path: Option<PathBuf>,
    /// True once any edit lands after the last save/load.
    pub dirty: bool,
    /// Display name (`modproject.json`'s `name`), for the hub and title bar.
    pub name: String,
}

/// Dirty-tracking model for the hub window. The app still tracks saves with
/// its own snapshot comparison; this model and its tests are the staged
/// replacement — wire the hub UI to it (or remove it) rather than letting the
/// two trackers drift.
#[allow(dead_code)]
impl CurrentProject {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_path(path: PathBuf, name: String) -> Self {
        Self {
            path: Some(path),
            dirty: false,
            name,
        }
    }

    #[allow(dead_code)]
    pub fn has_path(&self) -> bool {
        self.path.is_some()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Any edit lands here. Called by every mutation path (or, failing that,
    /// derived by comparing against the last saved snapshot).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// A successful save/load/export-adopt clears the flag and records the path.
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
    }

    /// Loading a project (or New) replaces the current one clean.
    #[allow(dead_code)]
    pub fn mark_loaded(&mut self, path: Option<PathBuf>, name: String) {
        self.path = path;
        self.name = name;
        self.dirty = false;
    }

    #[allow(dead_code)]
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Save needs a file dialog when there is no path yet.
    pub fn needs_save_as(&self) -> bool {
        self.path.is_none()
    }

    /// Switching projects with unsaved edits must warn first.
    pub fn needs_warning(&self) -> bool {
        self.dirty
    }
}

/// What the hub can do. Pure so the cold-start matrix is testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubAction {
    /// Reopen the last project path from config.
    ResumeLast(PathBuf),
    /// Start empty in a picked workspace folder. Carries the folder so every
    /// edit has a home from the first save — nothing lives only in memory.
    New(PathBuf),
    /// Open a `modproject.json` picked from disk.
    Open(PathBuf),
    /// Import a compiled mod folder as an editable project.
    ImportMod(PathBuf),
    /// Reopen one of the recent projects.
    OpenRecent(PathBuf),
    /// Dismiss the hub and keep browsing with no project.
    BrowseWithoutProject,
}

impl HubAction {
    /// Switching to this discards in-memory edits and must be guarded when dirty.
    /// Browsing without a project keeps the current in-memory edits, so it needs
    /// no guard; every other switch replaces them.
    pub fn discards_edits(&self) -> bool {
        !matches!(self, HubAction::BrowseWithoutProject)
    }

    #[allow(dead_code)]
    pub fn label(&self) -> String {
        match self {
            HubAction::ResumeLast(path) => format!("Resume {}", path.display()),
            HubAction::New(path) => format!("New project at {}", path.display()),
            HubAction::Open(path) => format!("Open {}", path.display()),
            HubAction::ImportMod(path) => format!("Import {}", path.display()),
            HubAction::OpenRecent(path) => format!("Open {}", path.display()),
            HubAction::BrowseWithoutProject => "Browse without project".to_string(),
        }
    }
}

/// True when choosing `action` with `dirty` unsaved edits must warn first.
pub fn needs_hub_warning(dirty: bool, action: &HubAction) -> bool {
    dirty && action.discards_edits()
}

/// The hub's cold-start state, derived from persisted config.
#[derive(Debug, Clone, Default)]
pub struct HubState {
    /// Whether the hub window is currently shown.
    pub show: bool,
    /// Last project path that still exists on disk, if any.
    pub last: Option<PathBuf>,
    /// Recent project paths that still exist, most-recent first.
    pub recent: Vec<PathBuf>,
}

impl HubState {
    /// Build from disk. Missing files are dropped so an unplugged drive does not
    /// leave a permanently failing Resume entry.
    pub fn cold_start(config_dir: &Path) -> Self {
        let last = load_last_project(config_dir).filter(|p| p.is_file());
        let mut recent = load_recent_projects(config_dir);
        recent.retain(|p| p.is_file());
        // The last project leads the recent list when it is also recent.
        Self {
            show: true,
            last,
            recent,
        }
    }

    /// The actions the hub offers right now, in display order.
    /// Same staged-model note as [`CurrentProject`]: tested, not yet wired.
    #[allow(dead_code)]
    pub fn choices(&self) -> Vec<HubChoice> {
        let mut out = Vec::new();
        if let Some(last) = &self.last {
            out.push(HubChoice::ResumeLast(last.clone()));
        }
        out.push(HubChoice::New);
        out.push(HubChoice::Open);
        out.push(HubChoice::ImportMod);
        for path in &self.recent {
            // Resume already covers the most recent path; listing it twice
            // invites opening it twice.
            if Some(path) != self.last.as_ref() {
                out.push(HubChoice::Recent(path.clone()));
            }
        }
        out.push(HubChoice::BrowseWithoutProject);
        out
    }
}

/// One row in the hub window. `Open`/`ImportMod` without a path mean "pick via
/// dialog"; the `HubAction` variants carry the picked path.
/// Same staged-model note as [`CurrentProject`]: tested, not yet wired.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubChoice {
    ResumeLast(PathBuf),
    New,
    Open,
    ImportMod,
    Recent(PathBuf),
    BrowseWithoutProject,
}

#[allow(dead_code)]
impl HubChoice {
    pub fn title(&self) -> &'static str {
        match self {
            HubChoice::ResumeLast(_) => "Resume last",
            HubChoice::New => "New",
            HubChoice::Open => "Open",
            HubChoice::ImportMod => "Import mod",
            HubChoice::Recent(_) => "Recent",
            HubChoice::BrowseWithoutProject => "Browse without project",
        }
    }
}

// ── Persistence (explicit config dir for testability) ───────────────────────

fn last_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LAST_PROJECT_KEY)
}

fn recent_path(config_dir: &Path) -> PathBuf {
    config_dir.join(RECENT_PROJECTS_KEY)
}

pub fn load_last_project(config_dir: &Path) -> Option<PathBuf> {
    let body = std::fs::read_to_string(last_path(config_dir)).ok()?;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

pub fn save_last_project(config_dir: &Path, path: &Path) {
    let _ = std::fs::create_dir_all(config_dir);
    let _ = std::fs::write(last_path(config_dir), path.to_string_lossy().as_bytes());
}

#[allow(dead_code)]
pub fn clear_last_project(config_dir: &Path) {
    let _ = std::fs::remove_file(last_path(config_dir));
}

pub fn load_recent_projects(config_dir: &Path) -> Vec<PathBuf> {
    let Ok(body) = std::fs::read_to_string(recent_path(config_dir)) else {
        return Vec::new();
    };
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn save_recent_projects(config_dir: &Path, recent: &[PathBuf]) {
    let _ = std::fs::create_dir_all(config_dir);
    let body = recent
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(recent_path(config_dir), body);
}

/// Record a project open/save: it becomes last and leads recents (deduped,
/// capped). Returns the new recent list.
pub fn push_recent(config_dir: &Path, path: &Path) -> Vec<PathBuf> {
    let mut recent = load_recent_projects(config_dir);
    recent.retain(|p| p != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(MAX_RECENT_PROJECTS);
    save_recent_projects(config_dir, &recent);
    save_last_project(config_dir, path);
    recent
}

/// Starting fresh (New / Browse without project) clears the last-project resume
/// but keeps recents: the user may still want to go back.
#[allow(dead_code)]
pub fn clear_current(config_dir: &Path) {
    clear_last_project(config_dir);
}

// ── Workspace: one folder that holds every edit ──────────────────────────
//
// A project is a folder, not just a file:
//   <workspace>/modproject.json   (every edit the tool supports)
//   <workspace>/assets/...        (portable textures/portraits, managed)
//   <workspace>/reference/...     (import reference copies, never exported)
//   <workspace>/romfs/...         (manual overlay, arc layout — models,
//                                  animations, and anything the tool does not
//                                  model; merged on export)
//
// New picks this folder upfront so nothing lives only in memory.

/// `modproject.json` inside a workspace folder.
pub fn workspace_file(workspace: &Path) -> PathBuf {
    workspace.join(crate::mod_project::PROJECT_FILE_NAME)
}

/// Manual overlay inside a workspace, in arc layout (`fighter/…`, `effect/…`).
pub fn workspace_romfs(workspace: &Path) -> PathBuf {
    workspace.join("romfs")
}

/// Files that make up one fighter model folder. A model edit normally needs more than the
/// `.numdlb`: material, skeleton, shader, helper, and texture files all travel together.
pub const MODEL_ASSET_EXTENSIONS: &[&str] = &[
    "adjb",
    "lvd",
    "nuhlpb",
    "numatb",
    "numdlb",
    "numshexb",
    "numshb",
    // `model.nuanmb` is the model's visibility/material animation. Motion animations are
    // imported into the separate motion directory below, so both uses must remain available.
    "nuanmb",
    "nusktb",
    "nusrcmdlb",
    "nutexb",
    "xmb",
];

/// A canonical file in the workspace overlay and the game path it will replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAssetFile {
    pub workspace_path: PathBuf,
    pub game_path: String,
}

/// Compact inventory shown in Project Files for the selected fighter/costume.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceAssetInventory {
    pub model_files: Vec<String>,
    pub animations: Vec<String>,
    pub swing_prc: bool,
    pub motion_list: bool,
}

impl WorkspaceAssetInventory {
    pub fn total_files(&self) -> usize {
        self.model_files.len()
            + self.animations.len()
            + usize::from(self.swing_prc)
            + usize::from(self.motion_list)
    }
}

fn fighter_component(fighter: &str) -> anyhow::Result<String> {
    let fighter = fighter.trim().to_ascii_lowercase();
    if fighter.is_empty()
        || !fighter
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("fighter name is not a safe game-path component: {fighter:?}");
    }
    Ok(fighter)
}

fn asset_file_name(source: &Path) -> anyhow::Result<String> {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.starts_with('.'))
        .ok_or_else(|| {
            anyhow::anyhow!("asset has no usable UTF-8 file name: {}", source.display())
        })?
        .to_ascii_lowercase();
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        bail!("asset has an unsafe file name: {name:?}");
    }
    Ok(name)
}

fn extension(source: &Path) -> String {
    source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_model_asset(source: &Path) -> bool {
    MODEL_ASSET_EXTENSIONS.contains(&extension(source).as_str())
}

fn is_animation_asset(source: &Path) -> bool {
    extension(source) == "nuanmb"
}

/// The size limits shared with the plugin's SD callback. Keeping the check here means a large
/// source file is rejected before it is copied into a generation snapshot.
pub const CARRIER_MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;
pub const CARRIER_MAX_TOTAL_SIZE: u64 = 256 * 1024 * 1024;

/// Alucard's model resource graph. The carrier has no safe fallback for these files: a custom
/// descriptor, mesh, or skeleton must be accompanied by the matching helper/visibility files.
pub const CARRIER_MODEL_FILES: &[&str] = &[
    "model.xmb",
    "model.nusktb",
    "model.numdlb",
    "model.numshb",
    "model.nuanmb",
    "model.numatb",
    "model.nuhlpb",
    "model.numshexb",
    "model.nusrcmdlb",
];

/// Canonical body-model and body-motion directories for one project costume.
pub fn workspace_fighter_slot_dirs(
    workspace: &Path,
    fighter: &str,
    slot: u8,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let fighter = fighter_component(fighter)?;
    let slot = format!("c{slot:02}");
    let base = workspace_romfs(workspace).join("fighter").join(fighter);
    Ok((
        base.join("model").join("body").join(&slot),
        base.join("motion").join("body").join(slot),
    ))
}

fn regular_asset_files(directory: &Path, kind: &str) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("reading {kind} folder {}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "{kind} source must be a regular, non-symlink folder: {}",
            directory.display()
        );
    }

    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("reading {kind} folder {}", directory.display()))?
    {
        let path = entry
            .with_context(|| format!("reading an entry in {}", directory.display()))?
            .path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading asset {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let supported = if kind == "model" {
            is_model_asset(&path)
        } else {
            is_animation_asset(&path)
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        matches!(
                            name.to_ascii_lowercase().as_str(),
                            "swing.prc" | "swingblend.prc" | "ik.prc" | "motion_list.bin"
                        )
                    })
        };
        if !supported {
            continue;
        }
        let name = asset_file_name(&path)?;
        if files.insert(name.clone(), path).is_some() {
            bail!("more than one {kind} asset has the name {name}");
        }
    }
    Ok(files)
}

/// Like `regular_asset_files`, but a missing folder reads as empty instead of failing.
/// Motion-side fallbacks rely on this: a model-only workspace legitimately has no motion
/// folder at all.
fn optional_asset_files(directory: &Path, kind: &str) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    if std::fs::symlink_metadata(directory)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(BTreeMap::new());
    }
    regular_asset_files(directory, kind)
}

fn checked_asset_size(path: &Path, label: &str) -> anyhow::Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("reading {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "{label} must be a regular, non-symlink file: {}",
            path.display()
        );
    }
    if metadata.len() == 0 {
        bail!("{label} is empty: {}", path.display());
    }
    if metadata.len() > CARRIER_MAX_FILE_SIZE {
        bail!(
            "{label} is {} bytes, above the {}-byte carrier file limit: {}",
            metadata.len(),
            CARRIER_MAX_FILE_SIZE,
            path.display()
        );
    }
    Ok(metadata.len())
}

fn source_for_model_file(
    files: &BTreeMap<String, PathBuf>,
    canonical_name: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(path) = files.get(canonical_name) {
        return Ok(path.clone());
    }
    let extension = canonical_name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_default();
    let candidates: Vec<_> = files
        .iter()
        .filter(|(name, _)| name.rsplit_once('.').map(|(_, ext)| ext) == Some(extension))
        .map(|(_, path)| path.clone())
        .collect();
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => bail!(
            "model package is missing {canonical_name}; include the complete Alucard-compatible model graph"
        ),
        _ => bail!(
            "model package has no {canonical_name} and contains multiple .{extension} files; rename the intended file to {canonical_name}"
        ),
    }
}

fn output_asset_path(
    output_dir: &Path,
    fighter: &str,
    slot: u8,
    area: &str,
    file: &str,
) -> WorkspaceAssetFile {
    let game_path = format!("fighter/{fighter}/{area}/body/c{slot:02}/{file}");
    WorkspaceAssetFile {
        workspace_path: output_dir.join(&game_path),
        game_path,
    }
}

fn write_prepared_file(source: &Path, destination: &Path, label: &str) -> anyhow::Result<u64> {
    checked_asset_size(source, label)?;
    replace_file_atomically(source, destination)
}

fn write_prepared_ssbh<T>(data: &T, destination: &Path, label: &str) -> anyhow::Result<u64>
where
    T: SsbhData,
    T::WriteError: std::error::Error + Send + Sync + 'static,
{
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("prepared asset has no parent folder"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating prepared asset folder {}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("prepared asset has no UTF-8 file name"))?;
    let staging = parent.join(format!(".{name}.visionary-next"));
    if staging.exists() {
        std::fs::remove_file(&staging)
            .with_context(|| format!("removing stale staging file {}", staging.display()))?;
    }
    data.write_to_file(&staging)
        .map_err(|error| anyhow::anyhow!("writing {label} {}: {error}", destination.display()))?;
    let size = checked_asset_size(&staging, label)?;
    if let Err(error) = std::fs::rename(&staging, destination) {
        let _ = std::fs::remove_file(&staging);
        return Err(error).with_context(|| format!("installing {label} {}", destination.display()));
    }
    Ok(size)
}

fn normalized_texture_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("material file contains an empty texture reference");
    }
    if name.starts_with('#') {
        return Ok(name.to_ascii_lowercase());
    }
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    Ok(name.strip_suffix(".nutexb").unwrap_or(&name).to_owned())
}

fn texture_source(files: &BTreeMap<String, PathBuf>, reference: &str) -> anyhow::Result<PathBuf> {
    let key = normalized_texture_name(reference)?;
    if key.starts_with('#') {
        bail!("internal texture reference {reference:?} does not have a .nutexb payload");
    }
    let file_name = format!("{key}.nutexb");
    files.get(&file_name).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "material references {reference:?}, but {file_name} is missing from the model folder"
        )
    })
}

/// Remap material references and their payloads together into native carrier entries.
fn carrier_texture_outputs(
    matl: &mut MatlData,
    files: &BTreeMap<String, PathBuf>,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut assigned: BTreeMap<String, String> = BTreeMap::new();
    let mut outputs = Vec::new();
    for entry in &mut matl.entries {
        for texture in &mut entry.textures {
            // Absolute shader textures are provided by the game's shared renderer.
            // Keep their full resource names instead of treating them as model siblings.
            if texture.data.starts_with("/common/shader/") {
                continue;
            }
            let key = normalized_texture_name(&texture.data)?;
            if key.starts_with('#') {
                continue;
            }
            let source = texture_source(files, &texture.data)?;
            let slot = if let Some(slot) = assigned.get(&key) {
                slot.clone()
            } else {
                let index = assigned.len();
                if index >= crate::carrier_support::pool::TEXTURE_CAPACITY {
                    bail!(
                        "texture graph exceeds the {} texture resource pool",
                        crate::carrier_support::pool::TEXTURE_CAPACITY
                    );
                }
                let slot = crate::carrier_support::pool::texture_name(index);
                assigned.insert(key, slot.clone());
                outputs.push((format!("{slot}.nutexb"), source));
                slot
            };
            texture.data = slot.to_owned();
        }
    }
    Ok(outputs)
}

fn validate_animation_bones(
    source: &Path,
    skeleton_bones: &HashSet<String>,
    label: &str,
) -> anyhow::Result<()> {
    let animation = AnimData::from_file(source)
        .map_err(|error| anyhow::anyhow!("reading {label} {}: {error}", source.display()))?;
    let mut unknown = BTreeSet::new();
    let mut mapped = 0usize;
    for group in animation.groups {
        if group.group_type != GroupType::Transform {
            continue;
        }
        for node in group.nodes {
            if !skeleton_bones.contains(&node.name) {
                unknown.insert(node.name);
            } else {
                mapped += 1;
            }
        }
    }
    if !unknown.is_empty() && mapped == 0 {
        let names = unknown.into_iter().take(8).collect::<Vec<_>>().join(", ");
        bail!("{label} references bone(s) not present in model.nusktb: {names}");
    }
    Ok(())
}

/// Change only the embedded texture name; preserve compressed image data and mipmaps.
fn rename_carrier_texture(path: &Path, name: &str) -> anyhow::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    if name.len() >= 64 || file.metadata()?.len() < 112 {
        bail!("invalid carrier texture: {}", path.display());
    }
    file.seek(SeekFrom::End(-112))?;
    let mut footer = [0u8; 112];
    file.read_exact(&mut footer)?;
    if &footer[..4] != b" XNT" || &footer[104..108] != b" XET" {
        bail!("invalid NUTEXB footer: {}", path.display());
    }
    footer[4..68].fill(0);
    footer[4..4 + name.len()].copy_from_slice(name.as_bytes());
    file.seek(SeekFrom::End(-112))?;
    file.write_all(&footer)?;
    Ok(())
}

/// What `prepare_carrier_assets` staged, plus where the motion side came from. Anything
/// listed in `vanilla_motion_used` was filled in from the vanilla dump because the workspace
/// motion folder did not provide it; `omitted_swing` means neither side had a `swing.prc`
/// (several fighters, Mario included, ship none) and the carrier proceeds without one.
/// `stripped_lod_meshes` counts far-range meshes omitted from the snapshot (see
/// [`is_low_lod_mesh`]); the workspace overlay and exports keep them.
#[derive(Debug, Clone)]
pub struct CarrierBundle {
    pub files: Vec<WorkspaceAssetFile>,
    pub vanilla_motion_used: Vec<String>,
    pub omitted_swing: bool,
    pub stripped_lod_meshes: usize,
}

/// Far-range LOD variant in Smash mesh naming: `<base>_lowShape` is the decimated twin of a
/// visibility-controlled base mesh. These twins carry no visibility tracks of their own and
/// rely on LOD selection, which a carrier-baked preview cannot reproduce faithfully — so a
/// preview built from them draws both ranges at once (hand and back weapons together).
fn is_low_lod_mesh(name: &str) -> bool {
    name.ends_with("_lowShape")
}

/// Omit far-range meshes from carrier descriptors. Returns how many descriptor entries were
/// removed. Whole names are filtered, so surviving entries keep their subindices.
fn strip_low_lod_entries(entries: &mut Vec<ssbh_data::modl_data::ModlEntryData>) -> usize {
    let before = entries.len();
    entries.retain(|entry| !is_low_lod_mesh(&entry.mesh_object_name));
    before - entries.len()
}

/// Prepare one immutable generation for the Alucard asset carrier.
///
/// The workspace's original overlay is never rewritten. The returned files are copies or
/// normalized SSBH descriptors under `output_dir/<game path>`, ready for the desktop to publish
/// as one generation-specific SD snapshot. Model filenames are normalized to Alucard's fixed
/// graph, material references and texture names are remapped to its native texture slots,
/// and the fighter motion list retains its identities while referencing pooled animations.
///
/// The model graph must come from the workspace in full. Motion is filled in: the workspace's
/// animation and `swing.prc` win when present, otherwise the same costume's vanilla files from
/// `vanilla_motion_dir` are used, so a model-only test needs no motion imports at all. When
/// neither side has a `swing.prc`, it is omitted rather than faked. This boundary stays
/// deliberate: a partial custom skeleton still cannot silently load Alucard's helper,
/// visibility animation, or physics data.
pub fn prepare_carrier_assets(
    workspace: &Path,
    fighter: &str,
    slot: u8,
    selected_animation: Option<&str>,
    vanilla_motion_dir: Option<&Path>,
    vanilla_model_dir: Option<&Path>,
    output_dir: &Path,
) -> anyhow::Result<CarrierBundle> {
    let fighter = fighter_component(fighter)?;
    let (model_dir, motion_dir) = workspace_fighter_slot_dirs(workspace, &fighter, slot)?;
    let mut model_files = BTreeMap::new();
    if let Some(vanilla) = vanilla_model_dir {
        model_files.extend(optional_asset_files(vanilla, "model")?);
    }
    model_files.extend(optional_asset_files(&model_dir, "model")?);
    let workspace_motion = optional_asset_files(&motion_dir, "motion")?;
    let mut motion_files = BTreeMap::new();
    if let Some(vanilla) = vanilla_motion_dir {
        for (name, path) in optional_asset_files(vanilla, "motion")? {
            motion_files.insert(name, path);
        }
    }
    for (name, path) in &workspace_motion {
        motion_files.insert(name.clone(), path.clone());
    }
    if model_files.is_empty() {
        bail!("model folder is empty: {}", model_dir.display());
    }

    let mut selected_model = BTreeMap::new();
    for &canonical in CARRIER_MODEL_FILES {
        // Body model animations are optional for fighters such as Hero. The carrier
        // still needs its reserved file, generated as an empty animation below.
        if canonical == "model.nuanmb" && !model_files.keys().any(|name| name.ends_with(".nuanmb"))
        {
            continue;
        }
        let source = source_for_model_file(&model_files, canonical)?;
        checked_asset_size(&source, canonical)?;
        selected_model.insert(canonical.to_string(), source);
    }

    let skeleton = SkelData::from_file(
        selected_model
            .get("model.nusktb")
            .expect("required skeleton was selected"),
    )
    .map_err(|error| anyhow::anyhow!("reading custom model skeleton: {error}"))?;
    let skeleton_bones: HashSet<String> = skeleton
        .bones
        .iter()
        .map(|bone| bone.name.clone())
        .collect();
    if skeleton_bones.is_empty() {
        bail!("custom model skeleton has no bones");
    }
    // The preview follows the complete vanilla motion set, so retain the vanilla bones.
    // Extra custom bones are fine (they simply idle under vanilla motion). Without the
    // vanilla skeleton there is nothing to check against, and an unverified rig must never
    // reach the fighter — dump that costume's model folder first.
    if let Some(vanilla_model) = vanilla_model_dir {
        let vanilla_skeleton_path = vanilla_model.join("model.nusktb");
        let vanilla_skeleton = SkelData::from_file(&vanilla_skeleton_path).map_err(|error| {
            anyhow::anyhow!(
                "reading vanilla skeleton {}: {error} — dump {fighter} c{slot:02}'s model folder to enable preview",
                vanilla_skeleton_path.display()
            )
        })?;
        let mut missing: Vec<&str> = vanilla_skeleton
            .bones
            .iter()
            .map(|bone| bone.name.as_str())
            .filter(|name| !skeleton_bones.contains(*name))
            .collect();
        missing.sort_unstable();
        if !missing.is_empty() {
            let shown = missing.into_iter().take(8).collect::<Vec<_>>().join(", ");
            bail!("custom skeleton is missing vanilla bone(s): {shown}; the live preview requires a compatible fighter rig");
        }
    }

    let mut modl = ModlData::from_file(
        selected_model
            .get("model.numdlb")
            .expect("required model descriptor was selected"),
    )
    .map_err(|error| anyhow::anyhow!("reading custom model.numdlb: {error}"))?;
    let mut command_modl = ModlData::from_file(
        selected_model
            .get("model.nusrcmdlb")
            .expect("required command model descriptor was selected"),
    )
    .map_err(|error| anyhow::anyhow!("reading custom model.nusrcmdlb: {error}"))?;
    let mut matl = MatlData::from_file(
        selected_model
            .get("model.numatb")
            .expect("required material descriptor was selected"),
    )
    .map_err(|error| anyhow::anyhow!("reading custom model.numatb: {error}"))?;
    if matl.entries.is_empty() {
        bail!("custom model.numatb has no material entries");
    }
    // The carrier has one material descriptor. Include each variant-only definition in
    // that descriptor so accepting a mesh label also supplies its material and textures.
    let mut material_labels: HashSet<String> = matl
        .entries
        .iter()
        .map(|entry| entry.material_label.clone())
        .collect();
    for (name, path) in model_files
        .iter()
        .filter(|(name, _)| name.ends_with(".numatb"))
    {
        if name == "model.numatb" {
            continue;
        }
        let Ok(variant) = MatlData::from_file(path) else {
            continue;
        };
        for entry in variant.entries {
            if material_labels.insert(entry.material_label.clone()) {
                matl.entries.push(entry);
            }
        }
    }
    for (label, descriptor) in [("model.numdlb", &modl), ("model.nusrcmdlb", &command_modl)] {
        if descriptor.material_file_names.len() != 1 {
            bail!(
                "{label} references {} material files; the Alucard carrier has one model.numatb slot",
                descriptor.material_file_names.len()
            );
        }
        for entry in &descriptor.entries {
            if !material_labels.contains(entry.material_label.as_str()) {
                bail!(
                    "{label} references material {:?}, which is absent from model.numatb and every other shipped .numatb",
                    entry.material_label
                );
            }
        }
    }
    // Normalize every path field that points at a sibling model resource. The selected source
    // files are all emitted under these exact names, regardless of how the imported package was
    // named on disk.
    for descriptor in [&mut modl, &mut command_modl] {
        descriptor.model_name = "model".to_owned();
        descriptor.skeleton_file_name = "model.nusktb".to_owned();
        descriptor.material_file_names = vec!["model.numatb".to_owned()];
        descriptor.animation_file_name = Some("model.nuanmb".to_owned());
        descriptor.mesh_file_name = "model.numshb".to_owned();
    }
    let texture_outputs = carrier_texture_outputs(&mut matl, &model_files)?;

    let selected_animation_name = selected_animation
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            if name.contains(['/', '\\']) {
                bail!("selected animation must be a file name, not a path: {name:?}");
            }
            let name = name.to_ascii_lowercase();
            if !name.ends_with(".nuanmb") || name == "model.nuanmb" {
                bail!("selected animation must be a motion .nuanmb file: {name:?}");
            }
            Ok(name)
        })
        .transpose()?
        .or_else(|| {
            motion_files
                .contains_key("wait.nuanmb")
                .then_some("wait.nuanmb".to_owned())
        })
        .or_else(|| {
            // Model-only test with no motion imports: prefer a vanilla wait pose, else any
            // motion animation. Whatever is picked is still bone-validated below.
            motion_files
                .keys()
                .filter(|name| name.ends_with(".nuanmb") && *name != "model.nuanmb")
                .min_by(|left, right| {
                    right
                        .contains("wait")
                        .cmp(&left.contains("wait"))
                        .then_with(|| left.cmp(right))
                })
                .cloned()
        })
        .ok_or_else(|| {
            anyhow::anyhow!("select a motion .nuanmb; neither the workspace nor the vanilla motion folder has one")
        })?;
    let animation_source = motion_files.get(&selected_animation_name).ok_or_else(|| {
        anyhow::anyhow!("selected animation is not in the motion folder: {selected_animation_name}")
    })?;
    checked_asset_size(animation_source, &selected_animation_name)?;
    validate_animation_bones(animation_source, &skeleton_bones, &selected_animation_name)?;
    if let Some(model_animation) = selected_model.get("model.nuanmb") {
        validate_animation_bones(model_animation, &skeleton_bones, "model.nuanmb")?;
    }

    let prepared_motions = crate::carrier_motion::prepare(&motion_files)?;
    for (name, path) in &prepared_motions.files {
        checked_asset_size(path, name)?;
        if workspace_motion.values().any(|source| source == path) {
            validate_animation_bones(path, &skeleton_bones, name)?;
        } else {
            // Vanilla motions can also animate optional effect/prop bones outside the body
            // skeleton. The native motion loader already handles these on the original
            // fighter; a compatible body rig must not reject its vanilla motion set.
            AnimData::from_file(path).map_err(|error| {
                anyhow::anyhow!("reading vanilla motion {}: {error}", path.display())
            })?;
        }
    }
    let swing_source = motion_files.get("swing.prc");
    let mut vanilla_motion_used = Vec::new();
    if !workspace_motion.contains_key(&selected_animation_name) {
        vanilla_motion_used.push(selected_animation_name.clone());
    }
    let mut omitted_swing = false;
    if let Some(swing_source) = swing_source {
        checked_asset_size(swing_source, "swing.prc")?;
        prc::open(swing_source).map_err(|error| {
            anyhow::anyhow!("reading swing.prc {}: {error}", swing_source.display())
        })?;
        if !workspace_motion.contains_key("swing.prc") {
            vanilla_motion_used.push("swing.prc".to_owned());
        }
    } else {
        // Fighters such as Mario ship no vanilla swing.prc; omitting it previews the model
        // with vanilla swing behavior instead of blocking the preview over a file the game
        // itself never had.
        omitted_swing = true;
    }
    if motion_files.contains_key("swingblend.prc")
        && !workspace_motion.contains_key("swingblend.prc")
    {
        vanilla_motion_used.push("swingblend.prc".to_owned());
    }
    if let Some(swingblend) = motion_files.get("swingblend.prc") {
        checked_asset_size(swingblend, "swingblend.prc")?;
        prc::open(swingblend).map_err(|error| {
            anyhow::anyhow!("reading swingblend.prc {}: {error}", swingblend.display())
        })?;
    }

    // Fighter attachment constraints (hands, sword, shield, etc.) belong to the
    // motion graph too. The carrier must use this costume's IK definitions.
    if let Some(ik) = motion_files.get("ik.prc") {
        checked_asset_size(ik, "ik.prc")?;
        prc::open(ik)
            .map_err(|error| anyhow::anyhow!("reading ik.prc {}: {error}", ik.display()))?;
        if !workspace_motion.contains_key("ik.prc") {
            vanilla_motion_used.push("ik.prc".to_owned());
        }
    }

    // Bound the complete source snapshot before writing anything. This also checks texture
    // payloads before descriptor serialization so an oversized texture cannot leave a large
    // partial generation behind.
    let mut source_bytes = 0u64;
    for source in selected_model
        .values()
        .chain(texture_outputs.iter().map(|(_, source)| source))
        .chain(prepared_motions.files.iter().map(|(_, path)| path))
        .chain(swing_source)
        .chain(motion_files.get("swingblend.prc"))
        .chain(motion_files.get("ik.prc"))
    {
        source_bytes = source_bytes
            .checked_add(checked_asset_size(source, "carrier source")?)
            .ok_or_else(|| anyhow::anyhow!("carrier source size overflowed"))?;
        if source_bytes > CARRIER_MAX_TOTAL_SIZE {
            bail!("carrier source bundle exceeds the {CARRIER_MAX_TOTAL_SIZE}-byte limit");
        }
    }

    // Far-range meshes bypass the visibility groups the preview transfers and rely on LOD
    // selection, which a carrier-baked instance cannot reproduce: it draws both ranges, so
    // hand and back weapons appear together. Omit them from the snapshot only (the overlay
    // and exports keep them). Files that fail to parse keep today's verbatim behavior along
    // with their descriptors, so an unfamiliar mesh blob can never half-strip a snapshot.
    let mut stripped_lod_meshes = 0usize;
    let mut rewritten_numshb = None;
    let mut rewritten_numshexb = None;
    if let (Some(numshb_source), Some(numshexb_source)) = (
        selected_model.get("model.numshb"),
        selected_model.get("model.numshexb"),
    ) {
        if let (Ok(mut mesh), Ok(mut meshex)) = (
            ssbh_data::mesh_data::MeshData::from_file(numshb_source),
            ssbh_data::meshex_data::MeshExData::from_file(numshexb_source),
        ) {
            let objects_before = mesh.objects.len();
            mesh.objects.retain(|object| !is_low_lod_mesh(&object.name));
            stripped_lod_meshes = objects_before - mesh.objects.len();
            meshex
                .mesh_object_groups
                .retain(|group| !is_low_lod_mesh(&group.mesh_object_full_name));
            if stripped_lod_meshes > 0 {
                strip_low_lod_entries(&mut modl.entries);
                strip_low_lod_entries(&mut command_modl.entries);
                rewritten_numshb = Some(mesh);
                rewritten_numshexb = Some(meshex);
            }
        }
    }

    let mut outputs = Vec::with_capacity(CARRIER_MODEL_FILES.len() + texture_outputs.len() + 3);
    let mut total_size = 0u64;
    let mut add_output = |output: WorkspaceAssetFile, size: u64| -> anyhow::Result<()> {
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("prepared carrier bundle size overflowed"))?;
        if total_size > CARRIER_MAX_TOTAL_SIZE {
            bail!(
                "prepared carrier bundle exceeds the {}-byte limit",
                CARRIER_MAX_TOTAL_SIZE
            );
        }
        outputs.push(output);
        Ok(())
    };
    for &canonical in CARRIER_MODEL_FILES {
        let output = output_asset_path(output_dir, &fighter, slot, "model", canonical);
        let size = match canonical {
            "model.nuanmb" if !selected_model.contains_key(canonical) => write_prepared_ssbh(
                &AnimData {
                    major_version: 2,
                    minor_version: 1,
                    final_frame_index: 0.0,
                    groups: Vec::new(),
                },
                &output.workspace_path,
                "empty model.nuanmb",
            )?,
            "model.numatb" => {
                write_prepared_ssbh(&matl, &output.workspace_path, "prepared model.numatb")?
            }
            "model.numdlb" => {
                write_prepared_ssbh(&modl, &output.workspace_path, "prepared model.numdlb")?
            }
            "model.nusrcmdlb" => write_prepared_ssbh(
                &command_modl,
                &output.workspace_path,
                "prepared model.nusrcmdlb",
            )?,
            "model.numshb" if rewritten_numshb.is_some() => write_prepared_ssbh(
                rewritten_numshb.as_ref().expect("stripped mesh was staged"),
                &output.workspace_path,
                "prepared model.numshb",
            )?,
            "model.numshexb" if rewritten_numshexb.is_some() => write_prepared_ssbh(
                rewritten_numshexb
                    .as_ref()
                    .expect("stripped mesh extra was staged"),
                &output.workspace_path,
                "prepared model.numshexb",
            )?,
            _ => write_prepared_file(
                selected_model
                    .get(canonical)
                    .expect("required model source was selected"),
                &output.workspace_path,
                canonical,
            )?,
        };
        add_output(output, size)?;
    }
    for (target_name, source) in texture_outputs {
        let output = output_asset_path(output_dir, &fighter, slot, "model", &target_name);
        let size = write_prepared_file(&source, &output.workspace_path, &target_name)?;
        rename_carrier_texture(
            &output.workspace_path,
            target_name
                .strip_suffix(".nutexb")
                .expect("texture extension"),
        )?;
        add_output(output, size)?;
    }
    for (name, source) in &prepared_motions.files {
        let output = output_asset_path(output_dir, &fighter, slot, "motion", name);
        let size = write_prepared_file(source, &output.workspace_path, name)?;
        add_output(output, size)?;
    }
    let list_output = output_asset_path(output_dir, &fighter, slot, "motion", "motion_list.bin");
    std::fs::create_dir_all(list_output.workspace_path.parent().unwrap())?;
    motion_lib::save(&list_output.workspace_path, &prepared_motions.list)?;
    let size = checked_asset_size(&list_output.workspace_path, "carrier motion list")?;
    add_output(list_output, size)?;
    let ik_output = output_asset_path(output_dir, &fighter, slot, "motion", "ik.prc");
    let ik_size = if let Some(ik) = motion_files.get("ik.prc") {
        write_prepared_file(ik, &ik_output.workspace_path, "ik.prc")?
    } else {
        // Do not inherit Alucard constraints when the fighter has none.
        prc::save(&ik_output.workspace_path, &prc::ParamStruct::default())?;
        checked_asset_size(&ik_output.workspace_path, "empty ik.prc")?
    };
    add_output(ik_output, ik_size)?;
    let swing_output = output_asset_path(output_dir, &fighter, slot, "motion", "swing.prc");
    let size = if let Some(swing_source) = swing_source {
        write_prepared_file(swing_source, &swing_output.workspace_path, "swing.prc")?
    } else {
        // A fighter without swing must not inherit Alucard's bone names and collisions.
        prc::save(&swing_output.workspace_path, &prc::ParamStruct::default())?;
        checked_asset_size(&swing_output.workspace_path, "empty swing.prc")?
    };
    add_output(swing_output, size)?;
    if let Some(swingblend) = motion_files.get("swingblend.prc") {
        let output = output_asset_path(output_dir, &fighter, slot, "motion", "swingblend.prc");
        add_output(
            output.clone(),
            write_prepared_file(swingblend, &output.workspace_path, "swingblend.prc")?,
        )?;
    }
    outputs.sort_by(|left, right| left.game_path.cmp(&right.game_path));
    Ok(CarrierBundle {
        files: outputs,
        vanilla_motion_used,
        omitted_swing,
        stripped_lod_meshes,
    })
}

/// Copy one regular file without ever exposing a half-written destination. Existing files are
/// moved aside until the replacement rename succeeds, which also makes this work on Windows.
pub fn replace_file_atomically(source: &Path, destination: &Path) -> anyhow::Result<u64> {
    let source_meta = std::fs::symlink_metadata(source)
        .with_context(|| format!("reading {}", source.display()))?;
    if source_meta.file_type().is_symlink() || !source_meta.is_file() {
        bail!(
            "asset must be a regular, non-symlink file: {}",
            source.display()
        );
    }
    if destination.exists() {
        let destination_meta = std::fs::symlink_metadata(destination)
            .with_context(|| format!("reading {}", destination.display()))?;
        if destination_meta.file_type().is_symlink() || !destination_meta.is_file() {
            bail!(
                "asset destination must be a regular, non-symlink file: {}",
                destination.display()
            );
        }
        if source.canonicalize().ok() == destination.canonicalize().ok() {
            return Ok(source_meta.len());
        }
    }

    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("asset destination has no parent"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("asset destination has no UTF-8 file name"))?;
    let staging = parent.join(format!(".{name}.visionary-next"));
    let previous = parent.join(format!(".{name}.visionary-previous"));
    if staging.exists() {
        std::fs::remove_file(&staging)
            .with_context(|| format!("removing stale staging file {}", staging.display()))?;
    }
    std::fs::copy(source, &staging).with_context(|| {
        format!(
            "copying {} to staging file {}",
            source.display(),
            staging.display()
        )
    })?;

    if destination.exists() {
        if previous.exists() {
            std::fs::remove_file(&previous)
                .with_context(|| format!("removing stale backup {}", previous.display()))?;
        }
        std::fs::rename(destination, &previous).with_context(|| {
            format!(
                "moving the previous asset out of the way: {}",
                destination.display()
            )
        })?;
        if let Err(error) = std::fs::rename(&staging, destination) {
            let _ = std::fs::rename(&previous, destination);
            let _ = std::fs::remove_file(&staging);
            return Err(error).with_context(|| {
                format!("installing replacement asset {}", destination.display())
            });
        }
        let _ = std::fs::remove_file(previous);
    } else if let Err(error) = std::fs::rename(&staging, destination) {
        let _ = std::fs::remove_file(&staging);
        return Err(error).with_context(|| format!("installing asset {}", destination.display()));
    }
    Ok(source_meta.len())
}

fn import_named_assets(
    sources: &[PathBuf],
    destination: &Path,
    accepts: fn(&Path) -> bool,
    kind: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    if sources.is_empty() {
        bail!("no {kind} files were selected");
    }
    let mut planned = Vec::with_capacity(sources.len());
    let mut names = std::collections::HashSet::new();
    for source in sources {
        if !accepts(source) {
            bail!("unsupported {kind} file: {}", source.display());
        }
        let name = asset_file_name(source)?;
        if !names.insert(name.clone()) {
            bail!("more than one selected {kind} file is named {name}");
        }
        planned.push((source, destination.join(name)));
    }
    let mut imported = Vec::with_capacity(planned.len());
    for (source, output) in planned {
        replace_file_atomically(source, &output)?;
        imported.push(output);
    }
    imported.sort();
    Ok(imported)
}

/// Import selected model-package files into the canonical project overlay.
pub fn import_model_files(
    workspace: &Path,
    fighter: &str,
    slot: u8,
    sources: &[PathBuf],
) -> anyhow::Result<Vec<PathBuf>> {
    let (model_dir, _) = workspace_fighter_slot_dirs(workspace, fighter, slot)?;
    import_named_assets(sources, &model_dir, is_model_asset, "model")
}

/// Import every recognized, top-level model-package file from a folder. Subdirectories and
/// symlinks are ignored so a mistaken folder choice cannot copy an unrelated tree.
pub fn import_model_folder(
    workspace: &Path,
    fighter: &str,
    slot: u8,
    source_dir: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
    let metadata = std::fs::symlink_metadata(source_dir)
        .with_context(|| format!("reading model folder {}", source_dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "model source must be a regular, non-symlink folder: {}",
            source_dir.display()
        );
    }
    let mut sources: Vec<PathBuf> = std::fs::read_dir(source_dir)
        .with_context(|| format!("reading model folder {}", source_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|meta| {
                meta.is_file() && !meta.file_type().is_symlink() && is_model_asset(path)
            })
        })
        .collect();
    sources.sort();
    import_model_files(workspace, fighter, slot, &sources)
}

/// Import one or more motion animations into the canonical project overlay.
pub fn import_animation_files(
    workspace: &Path,
    fighter: &str,
    slot: u8,
    sources: &[PathBuf],
) -> anyhow::Result<Vec<PathBuf>> {
    let (_, motion_dir) = workspace_fighter_slot_dirs(workspace, fighter, slot)?;
    import_named_assets(sources, &motion_dir, is_animation_asset, "animation")
}

/// Import a swing parameter file. The picked file may carry an iteration suffix locally; it is
/// installed under the game-required `swing.prc` name in the motion folder.
pub fn import_swing_prc(
    workspace: &Path,
    fighter: &str,
    slot: u8,
    source: &Path,
) -> anyhow::Result<PathBuf> {
    if extension(source) != "prc" {
        bail!("swing parameters must be a .prc file: {}", source.display());
    }
    let (_, motion_dir) = workspace_fighter_slot_dirs(workspace, fighter, slot)?;
    let destination = motion_dir.join("swing.prc");
    replace_file_atomically(source, &destination)?;
    Ok(destination)
}

/// Scan the canonical model/motion folders. This is deliberately derived from files instead of
/// serialized state: editing a file in Blender or an animation tool is visible immediately.
pub fn workspace_asset_inventory(
    workspace: &Path,
    fighter: &str,
    slot: u8,
) -> anyhow::Result<WorkspaceAssetInventory> {
    let mut inventory = WorkspaceAssetInventory::default();
    for file in workspace_asset_files(workspace, fighter, slot)? {
        let name = file.game_path.rsplit('/').next().unwrap_or_default();
        if file.game_path.contains("/model/") {
            inventory.model_files.push(name.to_owned());
        } else {
            match name {
                "swing.prc" => inventory.swing_prc = true,
                "motion_list.bin" => inventory.motion_list = true,
                name if name.ends_with(".nuanmb") => inventory.animations.push(name.to_owned()),
                _ => {}
            }
        }
    }
    Ok(inventory)
}

/// Every imported asset plus the lowercase game path the plugin/export consumes.
pub fn workspace_asset_files(
    workspace: &Path,
    fighter: &str,
    slot: u8,
) -> anyhow::Result<Vec<WorkspaceAssetFile>> {
    let fighter = fighter_component(fighter)?;
    let (model_dir, motion_dir) = workspace_fighter_slot_dirs(workspace, &fighter, slot)?;
    let mut files = Vec::new();
    for (directory, area, accepts) in [
        (&model_dir, "model", is_model_asset as fn(&Path) -> bool),
        (&motion_dir, "motion", |path: &Path| {
            is_animation_asset(path)
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.eq_ignore_ascii_case("swing.prc")
                            || name.eq_ignore_ascii_case("motion_list.bin")
                    })
        }),
    ] {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if !accepts(&path)
                || !std::fs::symlink_metadata(&path)
                    .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
            {
                continue;
            }
            let name = asset_file_name(&path)?;
            files.push(WorkspaceAssetFile {
                workspace_path: path,
                game_path: format!("fighter/{fighter}/{area}/body/c{slot:02}/{name}"),
            });
        }
    }
    files.sort_by(|left, right| left.game_path.cmp(&right.game_path));
    Ok(files)
}

/// Whether an overlay contains at least one regular, non-symlink file.
pub fn workspace_romfs_has_files(workspace: &Path) -> bool {
    let overlay = workspace_romfs(workspace);
    let mut pending = vec![overlay];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            // Keep this in lockstep with `merge_romfs_overlay`: an overlay symlink is not a
            // portable project file, even when its target happens to be a regular file.
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_file() {
                return true;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    false
}

/// Carry the manual overlay when Save As or Export Project adopts a new workspace. Existing
/// files in the destination win and are reported, matching normal mod export behavior.
pub fn preserve_workspace_romfs(
    old_project_file: &Path,
    new_project_file: &Path,
) -> anyhow::Result<(usize, Vec<String>)> {
    let old_workspace = old_project_file.parent().unwrap_or_else(|| Path::new("."));
    let new_workspace = new_project_file.parent().unwrap_or_else(|| Path::new("."));
    if old_workspace == new_workspace
        || (old_workspace.canonicalize().ok().is_some()
            && old_workspace.canonicalize().ok() == new_workspace.canonicalize().ok())
    {
        return Ok((0, Vec::new()));
    }
    merge_romfs_overlay(
        &workspace_romfs(old_workspace),
        &workspace_romfs(new_workspace),
    )
}

const WORKSPACE_README: &str = "\
# Visionary project workspace\n\
\n\
This folder holds every edit for the project.\n\
\n\
- `modproject.json` — every edit the tool supports (moves, params, roster,\n\
  names, portraits, effect values/textures). Edited in Visionary, saved with\n\
  Ctrl+S.\n\
- `assets/` — portable images managed by Visionary. Keep beside the JSON.\n\
- `reference/` — import reference copies (source text, notes). Never exported.\n\
- `romfs/` — MANUAL overlay in arc layout. Drop files the tool does not model\n\
  here and they ship verbatim on export:\n\
  - models: `romfs/fighter/<fighter>/model/...` (*.numdlb, *.numshb, *.nusktb…)\n\
  - animations: `romfs/fighter/<fighter>/motion/...` (*.nuanmb, *.nushdb…)\n\
  - sound: `romfs/sound/...`\n\
  - binary effect/message overrides: `romfs/effect/...`, `romfs/ui/message/...`\n\
\n\
Project Files can import models, animations, and swing.prc into this overlay.\n\
Edit those imported copies, then use Reload asset preview to update the\n\
experimental in-game carrier preview. Export keeps the original asset names.\n\
\n\
See Windows → Project Files in Visionary for the full map.\n";

/// Create a workspace folder: `modproject.json` (if missing), `assets/`,
/// `reference/`, `romfs/` plus a README explaining the manual drop zones.
/// Returns the `modproject.json` path. Idempotent — rerunning keeps edits.
pub fn scaffold_workspace(workspace: &Path, name: &str) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(workspace)?;
    std::fs::create_dir_all(workspace.join("assets"))?;
    std::fs::create_dir_all(workspace.join("reference"))?;
    std::fs::create_dir_all(workspace.join("romfs"))?;
    let readme = workspace.join("README.txt");
    if !readme.is_file() {
        std::fs::write(&readme, WORKSPACE_README)?;
    }
    let file = workspace_file(workspace);
    if !file.is_file() {
        let project = crate::mod_project::ModProjectFile {
            version: crate::mod_project::PROJECT_VERSION,
            name: crate::mod_export::slugify(name),
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&project)?;
        std::fs::write(&file, json)?;
    }
    Ok(file)
}

/// Merge a workspace `romfs/` overlay into an exported mod folder.
///
/// Copies every file under `overlay` into `dest` preserving arc-relative
/// paths. Files the generated export already wrote are skipped (generated
/// wins) and returned as conflicts so the export reports them instead of
/// silently overwriting in either direction. Returns `(copied, skipped)`.
pub fn merge_romfs_overlay(overlay: &Path, dest: &Path) -> anyhow::Result<(usize, Vec<String>)> {
    let mut copied = 0usize;
    let mut skipped = Vec::new();
    if !overlay.is_dir() {
        return Ok((copied, skipped));
    }
    let mut stack = vec![overlay.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `metadata()` follows symlinks. The overlay is an export boundary, so inspect the
            // directory entry itself and never copy a link to data outside the workspace.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(overlay) else {
                continue;
            };
            let out = dest.join(rel);
            if out.exists() {
                skipped.push(
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &out)?;
            copied += 1;
        }
    }
    skipped.sort();
    skipped.dedup();
    Ok((copied, skipped))
}

/// How an edit kind is handled by the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSupport {
    /// Edited in Visionary, stored in `modproject.json` / `assets/`.
    Supported,
    /// Copied on import for reading, never exported.
    Reference,
    /// Not modelled: drop into `romfs/` overlay, ships verbatim on export.
    Manual,
}

/// One row of the workspace map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub kind: &'static str,
    pub support: WorkspaceSupport,
    /// Where it lives in the workspace (relative).
    pub workspace_path: &'static str,
    /// Example game path it ships as.
    pub game_path: &'static str,
    pub notes: &'static str,
}

/// Every edit class and where it goes — including the ones the tool does not
/// support (models, animations, …). Single source for the Project Files panel.
pub fn workspace_map() -> Vec<WorkspaceEntry> {
    vec![
        WorkspaceEntry {
            kind: "Fighter moves (ACMD)",
            support: WorkspaceSupport::Supported,
            workspace_path: "modproject.json",
            game_path: "plugin.nro (built on export)",
            notes: "Hitboxes, effects, sounds per move.",
        },
        WorkspaceEntry {
            kind: "Fighter params",
            support: WorkspaceSupport::Supported,
            workspace_path: "modproject.json",
            game_path: "fighter/common/param/fighter_param.prc",
            notes: "Weight, speeds, jumps — sparse diffs.",
        },
        WorkspaceEntry {
            kind: "Roster order / visibility / row fields",
            support: WorkspaceSupport::Supported,
            workspace_path: "modproject.json",
            game_path: "ui/param/database/ui_chara_db.prc",
            notes: "Rebuilt from base + overrides on export.",
        },
        WorkspaceEntry {
            kind: "Display names",
            support: WorkspaceSupport::Supported,
            workspace_path: "modproject.json",
            game_path: "ui/message/msg_name.xmsbt",
            notes: "Adopted from .xmsbt on import.",
        },
        WorkspaceEntry {
            kind: "Portraits / stock icons",
            support: WorkspaceSupport::Supported,
            workspace_path: "assets/roster_ui/...",
            game_path: "ui/replace/chara/.../*.bntx",
            notes: "PNGs managed by Visionary.",
        },
        WorkspaceEntry {
            kind: "Effect textures",
            support: WorkspaceSupport::Supported,
            workspace_path: "assets/textures/...",
            game_path: "effect pool BNTX",
            notes: "Replacements + additions.",
        },
        WorkspaceEntry {
            kind: "Source reference",
            support: WorkspaceSupport::Reference,
            workspace_path: "reference/...",
            game_path: "—",
            notes: "Copied on import for reading; never exported.",
        },
        WorkspaceEntry {
            kind: "Models",
            support: WorkspaceSupport::Manual,
            workspace_path: "romfs/fighter/<fighter>/model/...",
            game_path: "fighter/<fighter>/model/...",
            notes: "Not modelled — drop .numdlb/.numshb/.nusktb here; ships verbatim.",
        },
        WorkspaceEntry {
            kind: "Animations",
            support: WorkspaceSupport::Manual,
            workspace_path: "romfs/fighter/<fighter>/motion/...",
            game_path: "fighter/<fighter>/motion/...",
            notes: "Not modelled — drop .nuanmb/.nusmab here; ships verbatim.",
        },
        WorkspaceEntry {
            kind: "Sound / streams",
            support: WorkspaceSupport::Manual,
            workspace_path: "romfs/sound/...",
            game_path: "sound/...",
            notes: "Not modelled — ships verbatim.",
        },
        WorkspaceEntry {
            kind: "Binary effects / messages",
            support: WorkspaceSupport::Manual,
            workspace_path: "romfs/effect/... · romfs/ui/message/...",
            game_path: "effect/... · ui/message/*.msbt",
            notes: "Compiled .eff/.msbt are reference-only; manual overrides ship verbatim.",
        },
        WorkspaceEntry {
            kind: "Compiled plugins",
            support: WorkspaceSupport::Reference,
            workspace_path: "reference/...",
            game_path: "*.nro",
            notes: "Reference-only by design; editor shows vanilla scripts.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"{}").unwrap();
    }

    #[test]
    fn cold_start_with_no_config_offers_new_open_import_and_browse() {
        let dir = config();
        let hub = HubState::cold_start(dir.path());
        assert!(hub.show, "the hub must appear on launch");
        assert!(hub.last.is_none());
        assert!(hub.recent.is_empty());
        let titles: Vec<&str> = hub.choices().iter().map(|c| c.title()).collect();
        assert_eq!(
            titles,
            vec!["New", "Open", "Import mod", "Browse without project"]
        );
    }

    #[test]
    fn resume_leads_and_recent_skips_the_duplicate() {
        let dir = config();
        let a = dir.path().join("a/modproject.json");
        let b = dir.path().join("b/modproject.json");
        touch(&a);
        touch(&b);
        save_last_project(dir.path(), &a);
        save_recent_projects(dir.path(), &[a.clone(), b.clone()]);

        let hub = HubState::cold_start(dir.path());
        assert_eq!(hub.last.as_deref(), Some(a.as_path()));
        // Recent still holds both, but the choices list Resume once.
        let choices = hub.choices();
        assert!(matches!(&choices[0], HubChoice::ResumeLast(p) if p == &a));
        let recents: Vec<&PathBuf> = choices
            .iter()
            .filter_map(|c| match c {
                HubChoice::Recent(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(recents, vec![&b], "Resume must not duplicate its path");
    }

    #[test]
    fn missing_files_are_dropped_from_resume_and_recents() {
        let dir = config();
        let gone = dir.path().join("gone/modproject.json");
        save_last_project(dir.path(), &gone);
        save_recent_projects(dir.path(), &[gone]);
        let hub = HubState::cold_start(dir.path());
        assert!(
            hub.last.is_none(),
            "a missing Resume target must not linger"
        );
        assert!(hub.recent.is_empty());
    }

    #[test]
    fn save_clears_dirty_and_save_as_is_needed_only_without_a_path() {
        let mut current = CurrentProject::new();
        assert!(current.needs_save_as());
        assert!(!current.is_dirty());
        current.mark_dirty();
        assert!(current.needs_warning());
        let dir = config();
        let path = dir.path().join("modproject.json");
        current.mark_saved(path.clone());
        assert!(!current.is_dirty());
        assert!(!current.needs_save_as());
        assert_eq!(current.path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn hub_switch_warns_only_when_edits_would_be_discarded() {
        let dirty = true;
        assert!(needs_hub_warning(
            dirty,
            &HubAction::Open("/tmp/x.json".into())
        ));
        assert!(needs_hub_warning(dirty, &HubAction::New("/tmp/new".into())));
        assert!(needs_hub_warning(
            dirty,
            &HubAction::ImportMod("/tmp/mod".into())
        ));
        assert!(needs_hub_warning(
            dirty,
            &HubAction::ResumeLast("/tmp/x.json".into())
        ));
        assert!(!needs_hub_warning(dirty, &HubAction::BrowseWithoutProject));
        assert!(!needs_hub_warning(
            false,
            &HubAction::New("/tmp/new".into())
        ));
    }

    #[test]
    fn push_recent_dedupes_caps_and_adopts_last() {
        let dir = config();
        let paths: Vec<PathBuf> = (0..12)
            .map(|i| dir.path().join(format!("p{i}/modproject.json")))
            .collect();
        for p in &paths {
            push_recent(dir.path(), p);
        }
        let recent = load_recent_projects(dir.path());
        assert_eq!(recent.len(), MAX_RECENT_PROJECTS);
        assert_eq!(recent[0], paths[11]);
        // Reopening an older entry moves it to the front without duplicating.
        push_recent(dir.path(), &paths[0]);
        let recent = load_recent_projects(dir.path());
        assert_eq!(recent[0], paths[0]);
        assert_eq!(
            recent.iter().filter(|p| *p == &paths[0]).count(),
            1,
            "reopening must not duplicate the entry"
        );
        assert_eq!(
            load_last_project(dir.path()).as_deref(),
            Some(paths[0].as_path())
        );
    }

    #[test]
    fn save_round_trip_persists_path_and_recent() {
        let dir = config();
        let path = dir.path().join("my_mod/modproject.json");
        touch(&path);
        let mut current = CurrentProject::with_path(path.clone(), "my_mod".into());
        current.mark_dirty();
        // Save adopts the path and clears dirty; hub records it.
        current.mark_saved(path.clone());
        push_recent(dir.path(), &path);
        assert!(!current.is_dirty());

        let hub = HubState::cold_start(dir.path());
        assert_eq!(hub.last.as_deref(), Some(path.as_path()));
        assert!(hub.recent.contains(&path));
    }

    #[test]
    fn new_scaffolds_a_workspace_where_every_edit_has_a_home() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("my_mod");
        let file = scaffold_workspace(&workspace, "My Mod").unwrap();
        assert_eq!(file, workspace.join("modproject.json"));
        assert!(file.is_file(), "New must create modproject.json upfront");
        assert!(workspace.join("assets").is_dir());
        assert!(workspace.join("reference").is_dir());
        assert!(workspace.join("romfs").is_dir());
        assert!(workspace.join("README.txt").is_file());
        // Rerunning keeps edits (idempotent).
        std::fs::write(&file, b"{\"kept\":true}").unwrap();
        let again = scaffold_workspace(&workspace, "My Mod").unwrap();
        assert_eq!(again, file);
        assert_eq!(std::fs::read(&file).unwrap(), b"{\"kept\":true}");
    }

    #[test]
    fn workspace_map_names_manual_drop_zones_for_unsupported_edits() {
        let map = workspace_map();
        let kinds: Vec<&str> = map.iter().map(|e| e.kind).collect();
        for needed in [
            "Models",
            "Animations",
            "Sound / streams",
            "Binary effects / messages",
        ] {
            assert!(
                kinds.contains(&needed),
                "workspace map must show where {needed} go: {kinds:?}"
            );
        }
        let models = map.iter().find(|e| e.kind == "Models").unwrap();
        assert_eq!(models.support, WorkspaceSupport::Manual);
        assert!(models.workspace_path.contains("romfs/fighter"));
        assert!(models.notes.contains("ships verbatim"));
        let anims = map.iter().find(|e| e.kind == "Animations").unwrap();
        assert_eq!(anims.support, WorkspaceSupport::Manual);
        assert!(anims.workspace_path.contains("motion"));
        // Supported edits keep living in the JSON/assets.
        assert!(map
            .iter()
            .any(|e| e.support == WorkspaceSupport::Supported
                && e.workspace_path == "modproject.json"));
    }

    #[test]
    fn romfs_overlay_merges_manual_files_and_reports_generated_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = dir.path().join("ws/romfs");
        let dest = dir.path().join("mod");
        std::fs::create_dir_all(overlay.join("fighter/mario/model/body/c00")).unwrap();
        std::fs::create_dir_all(overlay.join("fighter/mario/motion/body/c00")).unwrap();
        std::fs::write(
            overlay.join("fighter/mario/model/body/c00/model.numdlb"),
            b"model",
        )
        .unwrap();
        std::fs::write(
            overlay.join("fighter/mario/motion/body/c00/attack.nuanmb"),
            b"anim",
        )
        .unwrap();
        // Generated export already wrote this one — manual must not silently win.
        std::fs::create_dir_all(dest.join("fighter/common/param")).unwrap();
        std::fs::write(
            dest.join("fighter/common/param/fighter_param.prc"),
            b"generated",
        )
        .unwrap();
        std::fs::create_dir_all(overlay.join("fighter/common/param")).unwrap();
        std::fs::write(
            overlay.join("fighter/common/param/fighter_param.prc"),
            b"manual",
        )
        .unwrap();

        let (copied, skipped) = merge_romfs_overlay(&overlay, &dest).unwrap();
        assert_eq!(copied, 2, "model + animation ship verbatim");
        assert_eq!(skipped, vec!["fighter/common/param/fighter_param.prc"]);
        assert!(dest
            .join("fighter/mario/model/body/c00/model.numdlb")
            .is_file());
        assert!(dest
            .join("fighter/mario/motion/body/c00/attack.nuanmb")
            .is_file());
        assert_eq!(
            std::fs::read(dest.join("fighter/common/param/fighter_param.prc")).unwrap(),
            b"generated",
            "generated wins; the conflict is reported, not overwritten"
        );
    }

    #[test]
    fn overlay_only_workspace_is_exportable_without_serialized_edits() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        scaffold_workspace(&workspace, "Model iteration").unwrap();
        let model = workspace.join("romfs/fighter/pickel/model/body/c07/model.numdlb");
        let animation = workspace.join("romfs/fighter/pickel/motion/body/c07/attack11.nuanmb");
        let swing = workspace.join("romfs/fighter/pickel/motion/body/c07/swing.prc");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::create_dir_all(animation.parent().unwrap()).unwrap();
        std::fs::write(&model, b"model").unwrap();
        std::fs::write(&animation, b"animation").unwrap();
        std::fs::write(&swing, b"swing").unwrap();

        // The JSON scaffold is intentionally empty; the overlay itself is sufficient export
        // content and must be copied to an installable mod folder.
        assert!(workspace_romfs_has_files(&workspace));
        let destination = dir.path().join("mod");
        let (copied, conflicts) =
            merge_romfs_overlay(&workspace_romfs(&workspace), &destination).unwrap();
        assert_eq!(copied, 3);
        assert!(conflicts.is_empty());
        for path in [
            "fighter/pickel/model/body/c07/model.numdlb",
            "fighter/pickel/motion/body/c07/attack11.nuanmb",
            "fighter/pickel/motion/body/c07/swing.prc",
        ] {
            assert!(destination.join(path).is_file(), "missing {path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn romfs_overlay_skips_symlinked_files() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.numdlb");
        std::fs::write(&outside, b"must not export").unwrap();
        let overlay = dir.path().join("workspace/romfs");
        std::fs::create_dir_all(&overlay).unwrap();
        symlink(&outside, overlay.join("model.numdlb")).unwrap();
        let destination = dir.path().join("mod");

        assert!(!workspace_romfs_has_files(overlay.parent().unwrap()));
        let (copied, conflicts) = merge_romfs_overlay(&overlay, &destination).unwrap();
        assert_eq!((copied, conflicts), (0, Vec::new()));
        assert!(!destination.exists());
    }

    #[test]
    fn model_animation_and_swing_imports_land_in_the_selected_fighter_slot() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let source_model = dir.path().join("edited_model");
        std::fs::create_dir_all(&source_model).unwrap();
        std::fs::write(source_model.join("MODEL.NUMDLB"), b"mesh-v1").unwrap();
        std::fs::write(source_model.join("model.nusktb"), b"skeleton").unwrap();
        std::fs::write(source_model.join("body_col.nutexb"), b"texture").unwrap();
        std::fs::write(source_model.join("notes.blend"), b"do not ship").unwrap();

        let imported = import_model_folder(&workspace, "PICKEL", 12, &source_model).unwrap();
        assert_eq!(imported.len(), 3);
        let (model, motion) = workspace_fighter_slot_dirs(&workspace, "pickel", 12).unwrap();
        assert_eq!(
            std::fs::read(model.join("model.numdlb")).unwrap(),
            b"mesh-v1"
        );
        assert_eq!(
            std::fs::read(model.join("model.nusktb")).unwrap(),
            b"skeleton"
        );
        assert!(!model.join("notes.blend").exists());

        let animation = dir.path().join("AttackAirN.NUANMB");
        std::fs::write(&animation, b"animation").unwrap();
        import_animation_files(&workspace, "pickel", 12, &[animation]).unwrap();
        let swing = dir.path().join("swing_iteration_4.prc");
        std::fs::write(&swing, b"swing").unwrap();
        import_swing_prc(&workspace, "pickel", 12, &swing).unwrap();

        assert_eq!(
            std::fs::read(motion.join("attackairn.nuanmb")).unwrap(),
            b"animation"
        );
        assert_eq!(std::fs::read(motion.join("swing.prc")).unwrap(), b"swing");
        let inventory = workspace_asset_inventory(&workspace, "pickel", 12).unwrap();
        assert_eq!(inventory.model_files.len(), 3);
        assert_eq!(inventory.animations, vec!["attackairn.nuanmb"]);
        assert!(inventory.swing_prc);
        assert_eq!(inventory.total_files(), 5);
    }

    #[test]
    fn workspace_asset_manifest_uses_lowercase_canonical_game_paths() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let source_model = dir.path().join("MODEL.NUMSHB");
        let source_anim = dir.path().join("Run.NUANMB");
        let source_swing = dir.path().join("physics.prc");
        std::fs::write(&source_model, b"shader").unwrap();
        std::fs::write(&source_anim, b"run").unwrap();
        std::fs::write(&source_swing, b"physics").unwrap();
        import_model_files(&workspace, "Kirby", 3, &[source_model]).unwrap();
        import_animation_files(&workspace, "Kirby", 3, &[source_anim]).unwrap();
        import_swing_prc(&workspace, "Kirby", 3, &source_swing).unwrap();

        let paths: Vec<String> = workspace_asset_files(&workspace, "KIRBY", 3)
            .unwrap()
            .into_iter()
            .map(|file| file.game_path)
            .collect();
        assert_eq!(
            paths,
            vec![
                "fighter/kirby/model/body/c03/model.numshb",
                "fighter/kirby/motion/body/c03/run.nuanmb",
                "fighter/kirby/motion/body/c03/swing.prc",
            ]
        );
    }

    fn write_carrier_fixture(workspace: &Path) {
        use ssbh_data::anim_data::{
            AnimData, GroupData, NodeData, TrackData, TrackValues, Transform,
        };
        use ssbh_data::matl_data::{MatlData, MatlEntryData, ParamData, ParamId};
        use ssbh_data::modl_data::{ModlData, ModlEntryData};
        use ssbh_data::skel_data::{BillboardType, BoneData, SkelData};

        let (model, motion) = workspace_fighter_slot_dirs(workspace, "mario", 0).unwrap();
        std::fs::create_dir_all(&model).unwrap();
        std::fs::create_dir_all(&motion).unwrap();

        let skeleton = SkelData {
            major_version: 1,
            minor_version: 0,
            bones: vec![BoneData {
                name: "root".into(),
                transform: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                parent_index: None,
                billboard_type: BillboardType::Disabled,
            }],
        };
        skeleton.write_to_file(model.join("model.nusktb")).unwrap();

        let descriptor = ModlData {
            major_version: 1,
            minor_version: 7,
            model_name: "imported_model".into(),
            skeleton_file_name: "custom_skeleton.nusktb".into(),
            material_file_names: vec!["custom_materials.numatb".into()],
            animation_file_name: Some("custom_visibility.nuanmb".into()),
            mesh_file_name: "custom_mesh.numshb".into(),
            entries: vec![ModlEntryData {
                mesh_object_name: "mesh".into(),
                mesh_object_subindex: 0,
                material_label: "body_mat".into(),
            }],
        };
        descriptor
            .write_to_file(model.join("model.numdlb"))
            .unwrap();
        descriptor
            .write_to_file(model.join("model.nusrcmdlb"))
            .unwrap();

        let materials = MatlData {
            major_version: 1,
            minor_version: 6,
            entries: vec![MatlEntryData {
                material_label: "body_mat".into(),
                shader_label: "shader".into(),
                blend_states: Vec::new(),
                floats: Vec::new(),
                booleans: Vec::new(),
                vectors: Vec::new(),
                rasterizer_states: Vec::new(),
                samplers: Vec::new(),
                textures: vec![ParamData::new(ParamId::Texture0, "custom_diffuse".into())],
                uv_transforms: Vec::new(),
            }],
        };
        materials.write_to_file(model.join("model.numatb")).unwrap();
        let mut texture = vec![0u8; 119];
        texture[..7].copy_from_slice(b"texture");
        texture[7..11].copy_from_slice(b" XNT");
        texture[111..115].copy_from_slice(b" XET");
        std::fs::write(model.join("custom_diffuse.nutexb"), texture).unwrap();

        let model_animation = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 0.0,
            groups: Vec::new(),
        };
        model_animation
            .write_to_file(model.join("model.nuanmb"))
            .unwrap();
        let motion_animation = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 0.0,
            groups: vec![GroupData {
                group_type: GroupType::Transform,
                nodes: vec![NodeData {
                    name: "root".into(),
                    tracks: vec![TrackData {
                        name: "Transform".into(),
                        values: TrackValues::Transform(vec![Transform::IDENTITY]),
                        compensate_scale: false,
                        transform_flags: Default::default(),
                    }],
                }],
            }],
        };
        motion_animation
            .write_to_file(motion.join("edited.nuanmb"))
            .unwrap();
        let mut list = motion_lib::mlist::MList::default();
        list.list.insert(
            motion_lib::hash40::hash40("wait"),
            motion_lib::mlist::Motion {
                animations: vec![motion_lib::mlist::Animation {
                    name: motion_lib::hash40::hash40("edited.nuanmb"),
                    unk: 0,
                }],
                ..Default::default()
            },
        );
        motion_lib::save(motion.join("motion_list.bin"), &list).unwrap();
        prc::save(motion.join("swing.prc"), &prc::ParamStruct::default()).unwrap();

        // The remaining graph members are copied verbatim by the preparer. Their contents are
        // intentionally synthetic; format parsing is covered by the descriptor/skeleton/
        // material/animation checks above.
        for name in [
            "model.xmb",
            "model.numshb",
            "model.nuhlpb",
            "model.numshexb",
        ] {
            std::fs::write(model.join(name), name.as_bytes()).unwrap();
        }
    }

    #[test]
    fn carrier_preparation_maps_descriptors_textures_animation_and_swing_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let source_materials = workspace.join("romfs/fighter/mario/model/body/c00/model.numatb");
        let source_before = std::fs::read(&source_materials).unwrap();
        let output = dir.path().join("snapshot");
        std::fs::create_dir_all(&output).unwrap();

        let bundle = prepare_carrier_assets(
            &workspace,
            "MARIO",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &output,
        )
        .unwrap();
        let files = bundle.files;
        assert_eq!(
            files.len(),
            14,
            "nine model, one texture, animation, swing, IK, motion list"
        );
        assert!(files.iter().all(|file| file.workspace_path.is_file()));
        for file in &files {
            assert!(
                carrier_paths::validate_game_path("mario", &file.game_path).is_ok(),
                "prepared path rejected by plugin: {}",
                file.game_path
            );
            assert!(carrier_paths::validate_payload_path(
                1,
                &file.game_path,
                &format!("effect_viewer/live_assets/files/1/{}", file.game_path)
            )
            .is_ok());
        }
        assert!(files
            .iter()
            .any(|file| file.game_path.ends_with("/visionary_tex_0000.nutexb")));
        assert!(files
            .iter()
            .any(|file| file.game_path.ends_with("/visionary_motion_0000.nuanmb")));
        // The motion list accompanies its carrier-owned animation resources.
        assert!(files
            .iter()
            .any(|file| file.game_path.ends_with("/motion_list.bin")));
        assert_eq!(std::fs::read(source_materials).unwrap(), source_before);

        let prepared_materials =
            MatlData::from_file(output.join("fighter/mario/model/body/c00/model.numatb")).unwrap();
        assert_eq!(
            prepared_materials.entries[0].textures[0].data,
            crate::carrier_support::pool::texture_name(0)
        );
        let texture =
            std::fs::read(output.join("fighter/mario/model/body/c00/visionary_tex_0000.nutexb"))
                .unwrap();
        assert_eq!(&texture[..7], b"texture");
        assert_eq!(
            &texture[11..11 + crate::carrier_support::pool::texture_name(0).len()],
            crate::carrier_support::pool::texture_name(0).as_bytes()
        );
        let prepared_descriptor =
            ModlData::from_file(output.join("fighter/mario/model/body/c00/model.numdlb")).unwrap();
        assert_eq!(prepared_descriptor.model_name, "model");
        assert_eq!(prepared_descriptor.skeleton_file_name, "model.nusktb");
        assert_eq!(
            prepared_descriptor.material_file_names,
            vec!["model.numatb"]
        );
        assert_eq!(
            prepared_descriptor.animation_file_name.as_deref(),
            Some("model.nuanmb")
        );
        assert_eq!(prepared_descriptor.mesh_file_name, "model.numshb");
        assert_eq!(
            std::fs::read(
                output.join("fighter/mario/motion/body/c00/visionary_motion_0000.nuanmb")
            )
            .unwrap(),
            std::fs::read(workspace.join("romfs/fighter/mario/motion/body/c00/edited.nuanmb"))
                .unwrap()
        );
    }

    #[test]
    fn carrier_preparation_preserves_fighter_ik_and_prefers_workspace_edits() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let vanilla = dir.path().join("vanilla");
        std::fs::create_dir_all(&vanilla).unwrap();
        let fixture = |value| {
            prc::ParamStruct(vec![(
                hash40::Hash40::new("fixture"),
                prc::ParamKind::I32(value),
            )])
        };
        prc::save(vanilla.join("ik.prc"), &fixture(1)).unwrap();
        let output = dir.path().join("output");
        let bundle =
            prepare_carrier_assets(&workspace, "mario", 0, None, Some(&vanilla), None, &output)
                .unwrap();
        assert!(bundle.vanilla_motion_used.iter().any(|f| f == "ik.prc"));
        let target = output.join("fighter/mario/motion/body/c00/ik.prc");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            std::fs::read(vanilla.join("ik.prc")).unwrap()
        );
        let edited = workspace.join("romfs/fighter/mario/motion/body/c00/ik.prc");
        prc::save(&edited, &fixture(2)).unwrap();
        let bundle =
            prepare_carrier_assets(&workspace, "mario", 0, None, Some(&vanilla), None, &output)
                .unwrap();
        assert!(!bundle.vanilla_motion_used.iter().any(|f| f == "ik.prc"));
        assert_eq!(
            std::fs::read(&target).unwrap(),
            std::fs::read(edited).unwrap()
        );
    }

    #[test]
    fn carrier_preparation_preserves_shared_shader_texture_paths() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let path = workspace.join("romfs/fighter/mario/model/body/c00/model.numatb");
        let mut matl = MatlData::from_file(&path).unwrap();
        let shared = "/common/shader/sfxpbs/fighter/default_normal";
        matl.entries[0].textures[0].data = shared.into();
        matl.write_to_file(&path).unwrap();
        let bundle = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            None,
            None,
            None,
            &dir.path().join("output"),
        )
        .unwrap();
        let output = bundle
            .files
            .iter()
            .find(|f| f.game_path.ends_with("/model.numatb"))
            .unwrap();
        let prepared = MatlData::from_file(&output.workspace_path).unwrap();
        assert_eq!(prepared.entries[0].textures[0].data, shared);
        assert!(!bundle
            .files
            .iter()
            .any(|f| f.game_path.ends_with(".nutexb")));
    }

    #[test]
    fn carrier_preparation_supplies_absent_optional_model_animation() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let animation = workspace.join("romfs/fighter/mario/model/body/c00/model.nuanmb");
        std::fs::remove_file(&animation).unwrap();
        let bundle = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            None,
            None,
            None,
            &dir.path().join("output"),
        )
        .unwrap();
        let generated = bundle
            .files
            .iter()
            .find(|file| file.game_path.ends_with("/model.nuanmb"))
            .unwrap();
        let neutral = AnimData::from_file(&generated.workspace_path).unwrap();
        assert!(neutral.groups.is_empty());
        assert!(
            !animation.exists(),
            "preparation must not modify the imported model"
        );
    }

    #[test]
    fn low_lod_meshes_are_detected_and_stripped_by_name() {
        for name in ["blade_lowShape", "brave_sword_lowShape"] {
            assert!(is_low_lod_mesh(name), "{name}");
        }
        for name in [
            "mesh",
            "brave_sword_VIS_O_OBJShape",
            "lowShape",
            "blade_lowshape",
            "blade_lowShapeX",
            "Body",
        ] {
            assert!(!is_low_lod_mesh(name), "{name}");
        }
        use ssbh_data::modl_data::ModlEntryData;
        let mut entries = ["mesh", "blade_lowShape", "mesh", "shield_lowShape"]
            .into_iter()
            .enumerate()
            .map(|(index, mesh_object_name)| ModlEntryData {
                mesh_object_name: mesh_object_name.into(),
                mesh_object_subindex: (index % 2) as u64,
                material_label: "body_mat".into(),
            })
            .collect::<Vec<_>>();
        assert_eq!(strip_low_lod_entries(&mut entries), 2);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.mesh_object_name.as_str())
                .collect::<Vec<_>>(),
            ["mesh", "mesh"]
        );
    }

    #[test]
    fn carrier_preparation_omits_low_lod_meshes_from_the_snapshot_only() {
        use ssbh_data::mesh_data::{MeshData, MeshObjectData};
        use ssbh_data::meshex_data::MeshExData;
        use ssbh_data::modl_data::{ModlData, ModlEntryData};

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let model = workspace.join("romfs/fighter/mario/model/body/c00");
        // Descriptors, mesh objects, and mesh-extra groups each gain a far-range twin.
        for file in ["model.numdlb", "model.nusrcmdlb"] {
            let mut descriptor = ModlData::from_file(model.join(file)).unwrap();
            descriptor.entries.push(ModlEntryData {
                mesh_object_name: "blade_lowShape".into(),
                mesh_object_subindex: 0,
                material_label: "body_mat".into(),
            });
            descriptor.write_to_file(model.join(file)).unwrap();
        }
        let object = |name: &str| MeshObjectData {
            name: name.into(),
            subindex: 0,
            parent_bone_name: String::new(),
            sort_bias: 0,
            disable_depth_write: false,
            disable_depth_test: false,
            vertex_indices: Vec::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            binormals: Vec::new(),
            tangents: Vec::new(),
            texture_coordinates: Vec::new(),
            color_sets: Vec::new(),
            bone_influences: Vec::new(),
        };
        let mesh = MeshData {
            major_version: 1,
            minor_version: 8,
            objects: vec![object("mesh"), object("blade_lowShape")],
        };
        mesh.write_to_file(model.join("model.numshb")).unwrap();
        MeshExData::from_mesh_objects(&mesh.objects)
            .write_to_file(model.join("model.numshexb"))
            .unwrap();

        let bundle = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("output"),
        )
        .unwrap();
        assert_eq!(bundle.stripped_lod_meshes, 1);
        assert_eq!(bundle.files.len(), 14);
        // The workspace sources keep their far-range twins; only the snapshot drops them.
        assert!(ModlData::from_file(model.join("model.numdlb"))
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.mesh_object_name == "blade_lowShape"));
        let staged = dir.path().join("output/fighter/mario/model/body/c00");
        assert!(ModlData::from_file(staged.join("model.numdlb"))
            .unwrap()
            .entries
            .iter()
            .all(|entry| !is_low_lod_mesh(&entry.mesh_object_name)));
        assert!(ModlData::from_file(staged.join("model.nusrcmdlb"))
            .unwrap()
            .entries
            .iter()
            .all(|entry| !is_low_lod_mesh(&entry.mesh_object_name)));
        assert!(MeshData::from_file(staged.join("model.numshb"))
            .unwrap()
            .objects
            .iter()
            .all(|object| !is_low_lod_mesh(&object.name)));
        assert!(MeshExData::from_file(staged.join("model.numshexb"))
            .unwrap()
            .mesh_object_groups
            .iter()
            .all(|group| !is_low_lod_mesh(&group.mesh_object_full_name)));
    }

    #[test]
    #[ignore = "requires a local asset workspace and vanilla dump"]
    fn carrier_local_asset_smoke() {
        let workspace = PathBuf::from(std::env::var_os("VISIONARY_ASSET_WORKSPACE").unwrap());
        let vanilla = PathBuf::from(std::env::var_os("VISIONARY_ASSET_VANILLA").unwrap());
        let output = PathBuf::from(std::env::var_os("VISIONARY_ASSET_OUTPUT").unwrap());
        let fighter = std::env::var("VISIONARY_ASSET_FIGHTER").unwrap();
        let slot: u8 = std::env::var("VISIONARY_ASSET_SLOT")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .unwrap();
        let slot_component = format!("c{slot:02}");
        let bundle = prepare_carrier_assets(
            &workspace,
            &fighter,
            slot,
            None,
            Some(&vanilla.join("motion/body").join(&slot_component)),
            Some(&vanilla.join("model/body").join(&slot_component)),
            &output,
        )
        .unwrap();
        let textures: Vec<_> = bundle
            .files
            .iter()
            .filter(|f| f.game_path.ends_with(".nutexb"))
            .collect();
        for file in &textures {
            let texture = nutexb::NutexbFile::read_from_file(&file.workspace_path).unwrap();
            assert!(!texture.data.is_empty());
        }
        let list_file = bundle
            .files
            .iter()
            .find(|f| f.game_path.ends_with("/motion_list.bin"))
            .unwrap();
        let list = motion_lib::open(&list_file.workspace_path).unwrap();
        assert!(!list.list.is_empty());
        eprintln!(
            "prepared {} files, {} textures, {} motions",
            bundle.files.len(),
            textures.len(),
            list.list.len()
        );
    }

    #[test]
    fn carrier_preparation_rejects_partial_models_and_unknown_animation_bones() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        std::fs::remove_file(workspace.join("romfs/fighter/mario/model/body/c00/model.numshb"))
            .unwrap();
        let error = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("out"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("model.numshb"), "unexpected error: {error}");

        write_carrier_fixture(&workspace);
        use ssbh_data::anim_data::{
            AnimData, GroupData, NodeData, TrackData, TrackValues, Transform,
        };
        let unknown_animation = AnimData {
            major_version: 2,
            minor_version: 1,
            final_frame_index: 0.0,
            groups: vec![GroupData {
                group_type: GroupType::Transform,
                nodes: vec![NodeData {
                    name: "missing_bone".into(),
                    tracks: vec![TrackData {
                        name: "Transform".into(),
                        values: TrackValues::Transform(vec![Transform::IDENTITY]),
                        compensate_scale: false,
                        transform_flags: Default::default(),
                    }],
                }],
            }],
        };
        unknown_animation
            .write_to_file(workspace.join("romfs/fighter/mario/motion/body/c00/edited.nuanmb"))
            .unwrap();
        let error = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("out2"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("missing_bone"), "unexpected error: {error}");
    }

    #[test]
    fn carrier_preparation_fills_missing_motion_from_the_vanilla_dump() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        // Model-only workspace: the motion imports were never made.
        let edited =
            std::fs::read(workspace.join("romfs/fighter/mario/motion/body/c00/edited.nuanmb"))
                .unwrap();
        let swing =
            std::fs::read(workspace.join("romfs/fighter/mario/motion/body/c00/swing.prc")).unwrap();
        std::fs::remove_dir_all(workspace.join("romfs/fighter/mario/motion")).unwrap();
        let vanilla = dir.path().join("vanilla/fighter/mario/motion/body/c00");
        std::fs::create_dir_all(&vanilla).unwrap();
        std::fs::write(vanilla.join("a00wait1.nuanmb"), &edited).unwrap();
        let mut list = motion_lib::mlist::MList::default();
        list.list.insert(
            motion_lib::hash40::hash40("wait"),
            motion_lib::mlist::Motion {
                animations: vec![motion_lib::mlist::Animation {
                    name: motion_lib::hash40::hash40("a00wait1.nuanmb"),
                    unk: 0,
                }],
                ..Default::default()
            },
        );
        motion_lib::save(vanilla.join("motion_list.bin"), &list).unwrap();
        std::fs::write(vanilla.join("swing.prc"), &swing).unwrap();

        let output = dir.path().join("out");
        let bundle =
            prepare_carrier_assets(&workspace, "mario", 0, None, Some(&vanilla), None, &output)
                .unwrap();
        assert!(!bundle.omitted_swing);
        assert_eq!(
            bundle.vanilla_motion_used,
            vec!["a00wait1.nuanmb", "swing.prc"]
        );
        assert_eq!(
            std::fs::read(
                output.join("fighter/mario/motion/body/c00/visionary_motion_0000.nuanmb")
            )
            .unwrap(),
            edited
        );
        assert_eq!(
            std::fs::read(output.join("fighter/mario/motion/body/c00/swing.prc")).unwrap(),
            swing
        );
    }

    #[test]
    fn carrier_preparation_omits_swing_when_neither_side_has_one() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let edited =
            std::fs::read(workspace.join("romfs/fighter/mario/motion/body/c00/edited.nuanmb"))
                .unwrap();
        std::fs::remove_dir_all(workspace.join("romfs/fighter/mario/motion")).unwrap();
        // Mario-like dump: animation but no swing.prc anywhere.
        let vanilla = dir.path().join("vanilla/fighter/mario/motion/body/c00");
        std::fs::create_dir_all(&vanilla).unwrap();
        std::fs::write(vanilla.join("a00wait1.nuanmb"), &edited).unwrap();
        let mut list = motion_lib::mlist::MList::default();
        list.list.insert(
            motion_lib::hash40::hash40("wait"),
            motion_lib::mlist::Motion {
                animations: vec![motion_lib::mlist::Animation {
                    name: motion_lib::hash40::hash40("a00wait1.nuanmb"),
                    unk: 0,
                }],
                ..Default::default()
            },
        );
        motion_lib::save(vanilla.join("motion_list.bin"), &list).unwrap();

        let output = dir.path().join("out");
        let bundle =
            prepare_carrier_assets(&workspace, "mario", 0, None, Some(&vanilla), None, &output)
                .unwrap();
        assert!(bundle.omitted_swing);
        assert_eq!(bundle.vanilla_motion_used, vec!["a00wait1.nuanmb"]);
        assert!(output
            .join("fighter/mario/motion/body/c00/visionary_motion_0000.nuanmb")
            .is_file());
        let empty_swing =
            prc::open(output.join("fighter/mario/motion/body/c00/swing.prc")).unwrap();
        assert!(empty_swing.0.is_empty());
    }

    #[test]
    fn carrier_preparation_accepts_other_form_materials_from_variant_numatb() {
        use ssbh_data::matl_data::MatlEntryData;
        use ssbh_data::modl_data::ModlEntryData;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let model = workspace.join("romfs/fighter/mario/model/body/c00");

        // Another form's material lives only in its own variant file, like Wonder Mario's
        // metal-form material living in metamon_model.numatb rather than model.numatb.
        let mut command = ModlData::from_file(model.join("model.nusrcmdlb")).unwrap();
        command.entries.push(ModlEntryData {
            mesh_object_name: "form_mesh".into(),
            mesh_object_subindex: 0,
            material_label: "dark_mat".into(),
        });
        command
            .write_to_file(model.join("model.nusrcmdlb"))
            .unwrap();
        let variant = MatlData {
            major_version: 1,
            minor_version: 6,
            entries: vec![MatlEntryData {
                material_label: "dark_mat".into(),
                shader_label: "shader".into(),
                blend_states: Vec::new(),
                floats: Vec::new(),
                booleans: Vec::new(),
                vectors: Vec::new(),
                rasterizer_states: Vec::new(),
                samplers: Vec::new(),
                textures: Vec::new(),
                uv_transforms: Vec::new(),
            }],
        };
        variant
            .write_to_file(model.join("dark_model.numatb"))
            .unwrap();

        let bundle = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("out"),
        )
        .unwrap();
        let prepared = MatlData::from_file(
            dir.path()
                .join("out/fighter/mario/model/body/c00/model.numatb"),
        )
        .unwrap();
        assert!(prepared
            .entries
            .iter()
            .any(|entry| entry.material_label == "dark_mat"));
        assert!(!bundle.omitted_swing);
        assert!(bundle
            .files
            .iter()
            .any(|file| file.game_path.ends_with("/visionary_motion_0000.nuanmb")));
    }

    #[test]
    fn carrier_preparation_gates_foreign_rigs_against_the_vanilla_skeleton() {
        use ssbh_data::skel_data::{BillboardType, BoneData};

        fn skeleton_with(names: &[&str]) -> SkelData {
            SkelData {
                major_version: 1,
                minor_version: 0,
                bones: names
                    .iter()
                    .map(|name| BoneData {
                        name: (*name).into(),
                        transform: [
                            [1.0, 0.0, 0.0, 0.0],
                            [0.0, 1.0, 0.0, 0.0],
                            [0.0, 0.0, 1.0, 0.0],
                            [0.0, 0.0, 0.0, 1.0],
                        ],
                        parent_index: None,
                        billboard_type: BillboardType::Disabled,
                    })
                    .collect(),
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        // The fixture custom skeleton has exactly the bone "root".
        let vanilla = dir.path().join("vanilla_model");
        std::fs::create_dir_all(&vanilla).unwrap();
        skeleton_with(&["root"])
            .write_to_file(vanilla.join("model.nusktb"))
            .unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            Some(&vanilla),
            &out,
        )
        .unwrap();

        skeleton_with(&["root", "foreign_rig_bone"])
            .write_to_file(vanilla.join("model.nusktb"))
            .unwrap();
        let error = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            Some(&vanilla),
            &dir.path().join("out2"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("foreign_rig_bone"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn carrier_textures_use_expanded_graph_and_normalize_extensions() {
        assert_eq!(
            normalized_texture_name("folder/Albedo.NUTEXB").unwrap(),
            "albedo"
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let (model, _) = workspace_fighter_slot_dirs(&workspace, "mario", 0).unwrap();
        let mut matl = MatlData::from_file(model.join("model.numatb")).unwrap();
        let texture = matl.entries[0].textures[0].clone();
        // A complete model can exceed the old nine native slots.
        matl.entries[0].textures = (0..12)
            .map(|index| {
                let mut texture = texture.clone();
                texture.data = format!("texture{index}");
                std::fs::copy(
                    model.join("custom_diffuse.nutexb"),
                    model.join(format!("texture{index}.nutexb")),
                )
                .unwrap();
                texture
            })
            .collect();
        matl.write_to_file(model.join("model.numatb")).unwrap();

        let bundle = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("out"),
        )
        .unwrap();
        assert_eq!(
            bundle
                .files
                .iter()
                .filter(|f| f.game_path.ends_with(".nutexb"))
                .count(),
            12
        );
        let materials = MatlData::from_file(
            dir.path()
                .join("out/fighter/mario/model/body/c00/model.numatb"),
        )
        .unwrap();
        for (index, texture) in materials.entries[0].textures.iter().enumerate() {
            assert_eq!(
                texture.data,
                crate::carrier_support::pool::texture_name(index)
            );
        }
    }

    #[test]
    fn carrier_preparation_rejects_material_textures_missing_from_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        write_carrier_fixture(&workspace);
        let (model, _) = workspace_fighter_slot_dirs(&workspace, "mario", 0).unwrap();
        let mut matl = MatlData::from_file(model.join("model.numatb")).unwrap();
        let mut missing = matl.entries[0].textures[0].clone();
        missing.data = "ghost_texture".into();
        matl.entries[0].textures.push(missing);
        matl.write_to_file(model.join("model.numatb")).unwrap();

        let error = prepare_carrier_assets(
            &workspace,
            "mario",
            0,
            Some("edited.nuanmb"),
            None,
            None,
            &dir.path().join("out"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("ghost_texture.nutexb"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn imports_reject_path_traversal_symlinks_and_wrong_file_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let wrong = dir.path().join("model.blend");
        std::fs::write(&wrong, b"blend").unwrap();
        assert!(
            import_model_files(&workspace, "../kirby", 0, std::slice::from_ref(&wrong)).is_err()
        );
        assert!(import_model_files(&workspace, "kirby", 0, &[wrong]).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let real = dir.path().join("real.nuanmb");
            let linked = dir.path().join("linked.nuanmb");
            std::fs::write(&real, b"animation").unwrap();
            symlink(&real, &linked).unwrap();
            assert!(import_animation_files(&workspace, "kirby", 0, &[linked]).is_err());
        }
    }

    #[test]
    fn repeated_import_replaces_the_file_without_leaving_staging_debris() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let source = dir.path().join("model.numdlb");
        std::fs::write(&source, b"first").unwrap();
        let output = import_model_files(&workspace, "mario", 0, std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        std::fs::write(&source, b"second, longer").unwrap();
        import_model_files(&workspace, "mario", 0, &[source]).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"second, longer");
        assert!(!output
            .parent()
            .unwrap()
            .join(".model.numdlb.visionary-next")
            .exists());
        assert!(!output
            .parent()
            .unwrap()
            .join(".model.numdlb.visionary-previous")
            .exists());
    }

    #[test]
    fn save_as_preserves_the_romfs_overlay_and_keeps_destination_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let old_project = dir.path().join("old/modproject.json");
        let new_project = dir.path().join("new/modproject.json");
        touch(&old_project);
        touch(&new_project);
        let old_file = dir
            .path()
            .join("old/romfs/fighter/kirby/motion/body/c00/run.nuanmb");
        let old_conflict = dir
            .path()
            .join("old/romfs/fighter/kirby/motion/body/c00/swing.prc");
        let new_conflict = dir
            .path()
            .join("new/romfs/fighter/kirby/motion/body/c00/swing.prc");
        touch(&old_file);
        std::fs::write(&old_conflict, b"old").unwrap();
        if let Some(parent) = new_conflict.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&new_conflict, b"new").unwrap();

        assert!(workspace_romfs_has_files(old_project.parent().unwrap()));
        let (copied, skipped) = preserve_workspace_romfs(&old_project, &new_project).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(skipped, vec!["fighter/kirby/motion/body/c00/swing.prc"]);
        assert!(dir
            .path()
            .join("new/romfs/fighter/kirby/motion/body/c00/run.nuanmb")
            .is_file());
        assert_eq!(std::fs::read(new_conflict).unwrap(), b"new");
    }
}
