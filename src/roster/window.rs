//! The Roster window: mod library, character select, new characters, and traits.
//!
//! A separate viewport rather than a panel in the main window, for the same reason the Eff
//! Editor is one: it is a whole workspace, and the main window's space belongs to the move
//! being edited. `app.rs` owns one field of this type and one menu checkbox; everything else
//! about the roster lives under `src/roster/`.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use egui::{Color32, RichText, Ui};

use crate::ui::{act, look};

use super::css_view::CssView;
use super::index::RosterIndex;
use super::library::{self, ModLibrary, ModSource, PreparedMod, ProviderId};
use super::new_character::{self, NewCharacterAction, NewCharacterView};
use super::traits_view::TraitsView;
use super::RosterKey;
use crate::data::FighterEntry;
use crate::mod_project::{ParamMod, RosterMod};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterTab {
    Project,
    Library,
    CharacterSelect,
    NewCharacter,
    Traits,
}

impl RosterTab {
    const ALL: [Self; 5] = [
        Self::Project,
        Self::Library,
        Self::CharacterSelect,
        Self::NewCharacter,
        Self::Traits,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Project => "Characters",
            Self::Library => "Installed mods",
            Self::CharacterSelect => "Select screen",
            Self::NewCharacter => "New character",
            Self::Traits => "Fighter values",
        }
    }

    fn badge_count(self, window: &RosterWindow) -> Option<usize> {
        match self {
            Self::Project => None,
            Self::Library => {
                let conflicts = window
                    .library
                    .conflicts_by_fighter()
                    .values()
                    .map(|v| v.len())
                    .sum::<usize>();
                let stale = window.index.stale_overrides.len();
                let total = conflicts + stale;
                (total > 0).then_some(total)
            }
            Self::CharacterSelect => {
                let n = window.project.order.len()
                    + window.project.hidden.len()
                    + window.project.names.len()
                    + window.project.name_variants.len()
                    + window
                        .project
                        .per_costume_names
                        .values()
                        .map(|m| m.len())
                        .sum::<usize>()
                    + window
                        .project
                        .ui_images
                        .values()
                        .map(|m| m.len())
                        .sum::<usize>()
                    + window.project.chara_overrides.len();
                (n > 0).then_some(n)
            }
            Self::NewCharacter => {
                let n = window.project.authored.len();
                (n > 0).then_some(n)
            }
            Self::Traits => {
                let n: usize = window
                    .params
                    .values()
                    .map(|params| {
                        params
                            .files
                            .get(super::traits::FIGHTER_PARAM_PATH)
                            .map_or(0, std::collections::BTreeMap::len)
                    })
                    .sum();
                (n > 0).then_some(n)
            }
        }
    }
}

/// What one import attempt did, kept so the whole batch can be reported at once.
///
/// One bad archive in a multi-select must not abort the rest, and it must not vanish either:
/// a mod that silently failed to import looks exactly like a mod that imported and provides
/// nothing.
#[derive(Debug, Clone)]
pub struct ImportNote {
    pub source: String,
    pub outcome: Result<String, String>,
}

enum ImportMsg {
    Progress {
        done: usize,
        total: usize,
        name: String,
    },
    Prepared(Box<PreparedMod>),
    Failed {
        source: String,
        error: String,
    },
    Finished,
}

struct ImportJob {
    receiver: Receiver<ImportMsg>,
    done: usize,
    total: usize,
    current: String,
}

pub struct RosterWindow {
    pub open: bool,
    pub library: ModLibrary,
    /// The open project's roster edits. Authored here, read by `app.rs` when it builds the
    /// project file, and replaced wholesale when a project is loaded.
    pub project: RosterMod,
    /// The resolved roster. Rebuilt from scratch every frame the window is drawn rather than
    /// cached: the index is derived, and a cache is the mechanism by which a derived view
    /// drifts from the project it is supposed to be showing.
    index: RosterIndex,
    /// The character select tab. Owns the loaded roster database and the portrait cache,
    /// which are the only two things here expensive enough to be worth holding.
    css: CssView,
    /// The trait tab.
    traits: TraitsView,
    /// The new-character tab.
    new_character: NewCharacterView,
    /// Moves the project has replaced for each character you added, supplied by `app.rs` from
    /// the edit log. Without it the readiness panel would report every character as having
    /// replaced nothing.
    pub authored_moves: std::collections::BTreeMap<RosterKey, std::collections::BTreeSet<String>>,
    /// The character you added whose moveset is currently being edited, if any. Moves saved
    /// while this is set are scoped to that character's costume.
    edit_target: Option<RosterKey>,
    /// Sparse fighter-value edits, fighter name → edits. Read by `app.rs` when it builds the
    /// project file and replaced wholesale when a project is loaded, exactly like `project`.
    pub params: std::collections::BTreeMap<String, ParamMod>,
    /// Persisted constrained behavior graphs authored in the main editor.
    /// Keeping them here makes the project file and the visible editor share
    /// one session-owned source of truth.
    pub behaviors: std::collections::BTreeMap<String, crate::gimmicks::behavior::BehaviorGraph>,
    pub behavior_layouts:
        std::collections::BTreeMap<String, crate::gimmicks::field_canvas::FieldLayout>,
    /// Independent authored controllers, retained even while their field is closed.
    pub controller_charts: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, crate::mod_project::ControllerChart>,
    >,
    /// Persisted meter bindings, retained independently of whether their
    /// editor is currently open.
    pub meters: std::collections::BTreeMap<String, Vec<crate::gimmicks::meter::MeterBinding>>,
    tab: RosterTab,
    /// Portraits for the Characters cards.
    card_portraits: super::icons::PortraitCache,
    status: String,
    import: Option<ImportJob>,
    notes: Vec<ImportNote>,
    /// Set once the saved library has been loaded and pre-library mod roots adopted.
    initialized: bool,
    /// Set whenever the library changes in a way that invalidates the fighter index, so
    /// `app.rs` can rescan without this module reaching into it.
    pub library_dirty: bool,
    /// The mod whose file list is expanded in the library panel.
    expanded: Option<ProviderId>,
    /// The fighter whose conflicts are expanded.
    expanded_conflict: Option<String>,
    rename: Option<(ProviderId, String)>,
    /// Game roots in lookup order: the data root, then enabled mod roots in load order.
    /// Refreshed each frame from the library so a disabled mod stops providing files.
    roots: Vec<PathBuf>,
    /// ArcExplorer's export folder, which may contain parameter files absent
    /// from the main data root.
    export_root: Option<PathBuf>,
    /// hash40 → parameter field name, supplied by `app.rs` from the label download. The
    /// parameter files store hashes, and the full-field view has nothing to show without them.
    labels: std::collections::HashMap<u64, String>,
}

impl Default for RosterWindow {
    fn default() -> Self {
        Self {
            open: false,
            library: ModLibrary::default(),
            project: RosterMod::default(),
            index: RosterIndex::default(),
            css: CssView::default(),
            traits: TraitsView::default(),
            new_character: NewCharacterView::default(),
            authored_moves: Default::default(),
            edit_target: None,
            params: Default::default(),
            behaviors: Default::default(),
            behavior_layouts: Default::default(),
            controller_charts: Default::default(),
            meters: Default::default(),
            tab: RosterTab::Project,
            card_portraits: Default::default(),
            status: String::new(),
            import: None,
            notes: Vec::new(),
            initialized: false,
            library_dirty: false,
            expanded: None,
            expanded_conflict: None,
            rename: None,
            roots: Vec::new(),
            export_root: None,
            labels: Default::default(),
        }
    }
}

impl RosterWindow {
    /// Supply ArcExplorer's export folder as a read-only fallback for base
    /// game files. The main data root still wins when both contain a path.
    pub fn set_export_root(&mut self, root: Option<PathBuf>) {
        self.export_root = root;
    }

    pub fn set_parameter_labels(&mut self, labels: &std::collections::HashMap<u64, String>) {
        if self.labels != *labels {
            self.labels = labels.clone();
            self.traits.invalidate();
        }
    }

    /// Personal files resolve the winning enabled mod first, followed by base dumps.
    /// Main-editor controls and personal-file export must use the same baseline.
    pub fn gimmick_roots(&self, data_root: Option<&PathBuf>) -> Vec<PathBuf> {
        self.library
            .enabled()
            .rev()
            .map(|entry| entry.root.clone())
            .chain(data_root.into_iter().cloned())
            .chain(self.export_root.iter().cloned())
            .fold(Vec::new(), |mut roots, root| {
                if !roots.contains(&root) {
                    roots.push(root);
                }
                roots
            })
    }

    /// Preserve the existing shared roster/CSS lookup order independently.
    fn content_roots(&self, data_root: Option<&PathBuf>) -> Vec<PathBuf> {
        data_root
            .into_iter()
            .cloned()
            .chain(self.export_root.iter().cloned())
            .chain(self.library.enabled_roots())
            .fold(Vec::new(), |mut roots, root| {
                if !roots.contains(&root) {
                    roots.push(root);
                }
                roots
            })
    }

    /// Load the saved library and adopt any pre-library `mod_roots` entries.
    ///
    /// Deferred to first use rather than done at construction: it walks every mod's files, and
    /// a user who never opens the Roster window should not pay for that at startup.
    pub fn ensure_initialized(&mut self, legacy_roots: &[PathBuf]) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.library = library::load();
        let adopted = library::adopt_legacy_roots(&mut self.library, legacy_roots);
        if !adopted.is_empty() {
            self.status = format!("Adopted {} existing mod root(s).", adopted.len());
            self.library_dirty = true;
            library::save(&self.library);
        }
    }

    /// Rebuild the derived state: the lookup roots, the loaded roster database, and the index.
    ///
    /// Called before drawing and before exporting. Exporting has to do it too — the index is
    /// only meaningful once it has been built, and a project loaded with roster edits by a
    /// user who never opened this window would otherwise export with an empty index and
    /// report every one of its own edits as unwritable.
    fn refresh(&mut self, fighters: &[FighterEntry], data_root: Option<&PathBuf>) {
        self.roots = self.content_roots(data_root);
        self.css.ensure_loaded(&self.roots);
        self.index = RosterIndex::build(fighters, &self.library, &self.project, self.css.db());
    }

    /// Write this project's roster edits into an exported mod folder.
    ///
    /// Takes the project's roster section rather than reading `self.project`, so that an
    /// export always emits what the project file being written contains.
    pub fn export_into(
        &mut self,
        mod_root: &std::path::Path,
        roster: &RosterMod,
        fighters: &[FighterEntry],
        data_root: Option<&PathBuf>,
    ) -> anyhow::Result<super::export::RosterExport> {
        if roster.is_empty()
            && self
                .params
                .values()
                .all(crate::mod_project::ParamMod::is_empty)
        {
            return Ok(super::export::RosterExport::default());
        }
        self.ensure_initialized(&[]);
        self.refresh(fighters, data_root);
        let ui_root = self.ui_root();
        let mut report =
            super::export::export_roster(mod_root, ui_root.as_deref(), &self.index, roster)?;
        super::export::export_params(
            mod_root,
            self.param_root().as_deref(),
            &self.params,
            &self.labels,
            &mut report,
        )?;
        // Personal files are one per fighter rather than one shared file, so
        // this walks the same roots and patches each fighter's own clone.
        crate::gimmicks::export::export_personal(
            mod_root,
            &self.gimmick_roots(data_root),
            &self.params,
            &mut report,
        )?;
        super::export::export_authored_files(mod_root, &self.roots, &roster.authored, &mut report)?;
        super::export::export_costume_config(mod_root, &roster.authored, &mut report)?;
        Ok(report)
    }

    /// The arc root the shared fighter values file is read from, if one was found.
    ///
    /// Looked up separately from the roster database: `ui/` and `fighter/common/` are dumped
    /// independently, and a user who has one may well not have the other.
    fn param_root(&self) -> Option<PathBuf> {
        self.roots
            .iter()
            .find(|root| root.join(super::traits::FIGHTER_PARAM_PATH).is_file())
            .cloned()
    }

    /// The arc root the base roster database is read from, if one was found.
    fn ui_root(&self) -> Option<PathBuf> {
        super::css::locate_ui_root(&self.roots)
    }

    /// The costume slots new edits to `fighter` should belong to, if the user set one of their
    /// own characters as the edit target.
    ///
    /// Empty — the normal case — means an edit applies to every costume, which is what an
    /// ACMD replacement does. This is read on every edit save rather than latched, so turning
    /// the target off immediately stops scoping without a stale flag surviving anywhere.
    /// A multi-skin character returns all its slots, so one moveset covers c08–c15.
    pub fn slot_scopes_for(&self, fighter: &str) -> Vec<u8> {
        let Some(target) = self.edit_target.as_ref() else {
            return Vec::new();
        };
        let Some(entry) = self
            .project
            .authored
            .iter()
            .find(|authored| &authored.key == target)
        else {
            return Vec::new();
        };
        if !entry.donor.eq_ignore_ascii_case(fighter) {
            return Vec::new();
        }
        entry.all_slots()
    }

    /// The moves each added character has replaced, read out of the edit log.
    ///
    /// An edit belongs to a character when it is scoped to one of that character's
    /// costumes — the same fields the export gates on — so this cannot drift from
    /// what actually ships.
    pub fn replaced_moves(
        &self,
        log: &crate::data::EditLog,
    ) -> std::collections::BTreeMap<RosterKey, std::collections::BTreeSet<String>> {
        let mut out: std::collections::BTreeMap<RosterKey, std::collections::BTreeSet<String>> =
            Default::default();
        for authored in &self.project.authored {
            let Some(moves) = log.entries.get(&authored.donor) else {
                continue;
            };
            let owned = authored.all_slots();
            let replaced: std::collections::BTreeSet<String> = moves
                .iter()
                .filter(|(_, record)| {
                    let scopes = record.effective_scopes();
                    !scopes.is_empty() && scopes.iter().any(|slot| owned.contains(slot))
                })
                .map(|(move_name, _)| move_name.clone())
                .collect();
            if !replaced.is_empty() {
                out.insert(authored.key.clone(), replaced);
            }
        }
        out
    }

    /// Show the trait tab next time the window draws.
    pub fn focus_traits(&mut self) {
        self.tab = RosterTab::Traits;
    }

    /// Show one fighter's values.
    pub fn focus_traits_for(&mut self, fighter: &str) {
        self.tab = RosterTab::Traits;
        self.traits.select(super::RosterKey::fighter(fighter));
    }

    /// Show the installed mods.
    pub fn focus_library(&mut self) {
        self.tab = RosterTab::Library;
    }

    /// A character's select screen portrait for `slot`, with any picture the project gives it.
    pub fn portrait(
        &mut self,
        ctx: &egui::Context,
        key: &super::RosterKey,
        fighter: &str,
        slot: u8,
    ) -> Option<egui::TextureHandle> {
        let name_id = self
            .project
            .authored
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.name_id.clone())
            .or_else(|| self.index.by_key(key)?.name_id.clone())
            .or_else(|| {
                let entries = self.index.sorted();
                let entry = entries.iter().find(|entry| entry.fighter.as_deref() == Some(fighter))?;
                entry.name_id.clone()
            })
            // The Ice Climbers' select screen row is named for the pair, not either climber.
            .unwrap_or_else(|| match fighter {
                "popo" | "nana" => "ice_climber".to_owned(),
                _ => fighter.to_owned(),
            });
        let picked = self
            .project
            .ui_images
            .get(key)
            .and_then(|images| super::ui_images::find_override(images, "chara_1", slot, slot))
            .map(|picked| PathBuf::from(&picked.png_path));
        // A character made here files its portraits under its own colour numbers.
        let colour = self
            .project
            .authored
            .iter()
            .find(|entry| &entry.key == key)
            .map_or(slot, |entry| entry.all_slots().iter().position(|own| *own == slot).unwrap_or(0) as u8);
        let roots = self.roots.clone();
        self.card_portraits
            .get_with_gamma(ctx, &roots, &name_id, colour, true, picked.as_deref())?
            .ok()
            .cloned()
    }

    /// Show the character select tab next time the window draws.
    pub fn focus_character_select(&mut self) {
        self.tab = RosterTab::CharacterSelect;
    }

    /// Show the new-character tab next time the window draws.
    pub fn focus_new_character(&mut self) {
        self.tab = RosterTab::NewCharacter;
    }

    /// Jump to Character Select with one entry selected and its editor
    /// scrolled into view — the landing end of cross-tab "finish look" jumps.
    pub fn select_css(&mut self, key: crate::roster::RosterKey) {
        self.tab = RosterTab::CharacterSelect;
        self.css.select_key(key);
    }

    /// Clear every position edit (edit-log revert path): the sparse order
    /// map plus any typed drafts showing in the CSS editor.
    pub fn clear_positions(&mut self) {
        self.project.order.clear();
        self.css.clear_order_drafts();
    }

    /// Enabled mod roots in load order — what the fighter scan consumes.
    pub fn enabled_roots(&self) -> Vec<PathBuf> {
        self.library.enabled_roots()
    }

    /// Summary for the export tree: how many roster edits this project holds.
    pub fn export_summary(&self) -> RosterExportSummary {
        let order = self.project.order.len();
        let hidden = self.project.hidden.len();
        let names = self.project.names.len()
            + self.project.name_variants.len()
            + self
                .project
                .per_costume_names
                .values()
                .map(|m| m.len())
                .sum::<usize>()
            + self
                .project
                .ui_images
                .values()
                .map(|m| m.len())
                .sum::<usize>()
            + self.project.chara_overrides.len();
        let authored = self.project.authored.len();
        let params: usize = self.params.values().map(|p| p.field_count()).sum();
        RosterExportSummary {
            order,
            hidden,
            names,
            authored,
            params,
        }
    }

    /// Any project edit the Edit Log should open for — roster or trait
    /// values. A roster-only project otherwise shows a disabled log with no
    /// way to review or revert what it holds.
    pub fn has_edits(&self) -> bool {
        let summary = self.export_summary();
        summary.order + summary.hidden + summary.names + summary.authored + summary.params > 0
    }

    /// Keep the page's data current; call every frame the page may be shown.
    pub fn prepare(
        &mut self,
        ctx: &egui::Context,
        fighters: &[FighterEntry],
        data_root: Option<&PathBuf>,
        labels: &std::collections::HashMap<u64, String>,
    ) {
        if !self.open {
            return;
        }
        // Labels arrive asynchronously and grow; clone when contents differ rather
        // than only on length – two maps of the same length with different keys
        // would otherwise keep showing stale names.
        self.set_parameter_labels(labels);
        self.ensure_initialized(&[]);
        self.poll_import(ctx);
        self.refresh(fighters, data_root);
    }

    /// Whether the project has a folder on disk; new characters need one.
    /// The character move edits go to alone, when one is picked.
    pub fn edit_target(&self) -> Option<&RosterKey> {
        self.edit_target.as_ref()
    }

    pub fn set_project_dir(&mut self, dir: Option<PathBuf>) {
        self.new_character.project_dir = dir;
    }

    /// The Characters page, drawn in the main window.
    /// Draw the page; true when "This project" is showing, which the app draws below.
    pub fn page(&mut self, ui: &mut Ui) -> bool {
        self.card_portraits.begin_frame();
        self.ui_contents(ui);
        self.tab == RosterTab::Project
    }

    fn ui_contents(&mut self, ui: &mut Ui) {
        // Larger type for the whole roster workspace, applied once here so
        // every tab inherits it without touching each label. `.small()` stays
        // relative — it still reads as secondary text, just legibly so.
        // (The main move-editor window keeps its own denser scale.)
        {
            let style = ui.style_mut();
            for font in style.text_styles.values_mut() {
                font.size *= 1.18;
            }
            style.spacing.item_spacing.y = (style.spacing.item_spacing.y + 1.0).max(5.0);
            // Small and faint text stay readable: no hint-grey, nothing under 13 px.
            if let Some(small) = style.text_styles.get_mut(&egui::TextStyle::Small) {
                small.size = small.size.max(13.0);
            }
            let look = look(ui);
            let style = ui.style_mut();
            style.visuals.weak_text_color = Some(look.dim);
            style.visuals.widgets.noninteractive.fg_stroke.color = look.text;
        }
        // One slim top bar: tabs with badges, edit count, and (only when
        // there is something to ship) what an export will contain. The old
        // layout stacked a title card, a metrics row, and a tab-description
        // line above the actual workspace — the grid and the editor below
        // are the focus, not this.
        if self.tab == RosterTab::Project {
            return;
        }
        let look = look(ui);
        ui.painter().rect_filled(ui.max_rect(), 0.0, look.page);
        ui.add_space(4.0);
        self.draw_top_bar(ui, look);
        ui.add_space(10.0);

        // Content — one outer vertical scroll. Horizontal scrolling lives
        // inside the CSS grid only; the traits tab draws into this scroll
        // directly so two nested vertical scrollers never fight. Content-drag
        // scrolling is off here: the CSS tab owns reorder and marquee drags,
        // and a panning scroll area eats those (and sloppy clicks with them).
        egui::Frame::new()
            .fill(look.card)
            .stroke(egui::Stroke::new(1.5, look.edge))
            .corner_radius(14.0)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("roster_main_scroll")
                    .auto_shrink([false, false])
                    .scroll_source(super::GESTURE_AREA_SCROLL)
                    .show(ui, |ui| {
                        ui.set_max_width((ui.available_width() - 4.0).max(400.0));
                        match self.tab {
                            RosterTab::Project => {}
                            RosterTab::Library => self.draw_library(ui),
                            RosterTab::CharacterSelect => {
                                let roots = self.roots.clone();
                                self.css.ui(ui, &roots, &self.index, &mut self.project);
                                if self.css.take_goto_new_character() {
                                    self.tab = RosterTab::NewCharacter;
                                }
                            }
                            RosterTab::NewCharacter => {
                                let roots = self.roots.clone();
                                let fighters: Vec<String> = self.index.entries.iter().filter_map(|entry| entry.fighter.clone()).collect();
                                let faces: std::collections::HashMap<String, egui::TextureHandle> = fighters
                                    .into_iter()
                                    .filter_map(|fighter| {
                                        let face = self.portrait(ui.ctx(), &RosterKey::fighter(&fighter), &fighter, 0)?;
                                        Some((fighter, face))
                                    })
                                    .collect();
                                let mut view = std::mem::take(&mut self.new_character);
                                let action = view.ui(ui, &roots, &self.index, &self.project, &faces);
                                self.new_character = view;
                                self.handle_new_character(action);
                            }
                            RosterTab::Traits => {
                                let roots = self.roots.clone();
                                let labels = std::mem::take(&mut self.labels);
                                self.traits
                                    .ui(ui, &roots, &labels, &self.index, &mut self.params);
                                self.labels = labels;
                            }
                        }
                    });
            });

        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(&self.status).color(look.text));
        }
    }

    /// The page title, a way back to the project, and the other pages.
    fn draw_top_bar(&mut self, ui: &mut Ui, look: crate::ui::Look) {
        ui.horizontal_wrapped(|ui| {
            if act(ui, look, "Project", None).clicked() {
                self.tab = RosterTab::Project;
            }
            ui.add_space(8.0);
            ui.label(RichText::new(self.tab.title()).size(30.0).strong().color(look.text));
            ui.add_space(12.0);
            for tab in RosterTab::ALL {
                if tab == RosterTab::Project || tab == self.tab {
                    continue;
                }
                let mut text = tab.title().to_string();
                if let Some(n) = tab.badge_count(self) {
                    text.push_str(&format!(" ({n})"));
                }
                if act(ui, look, &text, None).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    /// Carry out what the new-character tab asked for.
    ///
    /// Performed here rather than inside the panel so that every mutation of the library and
    /// the project goes through one place — the library has to be saved and the fighter index
    /// invalidated on any change, and a second path that forgot either would leave the new
    /// character invisible until the next restart.
    pub fn handle_new_character(&mut self, action: NewCharacterAction) {
        match action {
            NewCharacterAction::None => {}
            NewCharacterAction::Create {
                donor,
                slots,
                display_name,
                name_id,
                destination,
            } => {
                // The new character starts as a full copy of the fighter it plays as.
                let root = destination.join(crate::mod_export::slugify(&display_name));
                let ui_name = self
                    .index
                    .by_key(&RosterKey::fighter(&donor))
                    .and_then(|entry| entry.name_id.clone())
                    .unwrap_or_else(|| donor.to_ascii_lowercase());
                let create_res = new_character::create_and_import_range(
                    &mut self.library,
                    &destination,
                    &self.roots,
                    &donor,
                    &ui_name,
                    &slots,
                    &display_name,
                    &name_id,
                );
                match create_res {
                    Ok((scaffolded, copied)) => {
                        let mut entry = new_character::authored_entry_multi(
                            &donor,
                            &slots,
                            &display_name,
                            &name_id,
                        );
                        // Remember where the files went: the panels reveal
                        // this folder and report on its contents. Computed the
                        // same way `create_and_import` computes it.
                        entry.files_root = Some(root);
                        // The name is recorded as a roster edit as well as on the entry, so it
                        // reaches the .xmsbt the export writes. The entry's own field is what
                        // the panel shows; the override is what the game reads.
                        self.project
                            .names
                            .insert(entry.key.clone(), display_name.clone());
                        self.project
                            .authored
                            .retain(|existing| existing.key != entry.key);
                        self.project.authored.push(entry);
                        self.new_character.note_created(&display_name, &scaffolded, copied);
                        self.commit_library_change();
                    }
                    Err(error) => self.new_character.note_error(format!("{error:#}")),
                }
            }
            NewCharacterAction::SetEditTarget(target) => {
                self.status = match &target {
                    Some(key) => {
                        let name = self.project.authored.iter().find(|entry| &entry.key == key).map_or(key.to_string(), |entry| entry.display_name.clone());
                        format!("Moves you edit now change only {name}.")
                    }
                    None => "Moves you edit now change every costume of the fighter again.".to_string(),
                };
                self.edit_target = target;
            }
            NewCharacterAction::Remove(key) => {
                if self.edit_target.as_ref() == Some(&key) {
                    self.edit_target = None;
                }
                self.project.authored.retain(|entry| entry.key != key);
                self.project.names.remove(&key);
                self.project.name_variants.remove(&key);
                self.project.order.remove(&key);
                self.project.hidden.remove(&key);
                self.project.ui_images.remove(&key);
                self.project.chara_overrides.remove(&key);
                self.status = "Removed the character from the project. Its files are still in its folder.".to_string();
            }
        }
    }

    // ── Library tab ─────────────────────────────────────────────────────────

    fn draw_library(&mut self, ui: &mut Ui) {
        let look = look(ui);
        ui.horizontal(|ui| {
            if act(ui, look, "Add mod folder", Some(look.accent))
                .on_hover_text("A folder with fighter, effect or ui in it, or one that holds such a folder")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new().set_title("Add mod folder").pick_folder() {
                    self.start_import(vec![path]);
                }
            }
            if act(ui, look, "Add mod archives", None)
                .on_hover_text(".zip or .7z files; Visionary unpacks them for you")
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new()
                    .set_title("Add mod archives")
                    .add_filter("Mod archive", super::archive::SUPPORTED)
                    .pick_files()
                {
                    self.start_import(paths);
                }
            }
        });
        ui.add_space(8.0);

        if let Some(job) = &self.import {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Adding {} of {}: {}", job.done + 1, job.total, job.current)).strong().color(look.text));
            });
            let frac = if job.total > 0 { job.done as f32 / job.total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(ui.available_width().min(360.0)).desired_height(6.0));
            ui.add_space(8.0);
        }

        self.draw_notes(ui);

        if self.library.is_empty() {
            ui.label(RichText::new("No mods added yet. Mods you add here show up in the fighter list with their skins and effects.").color(look.dim));
            self.draw_stale_overrides(ui);
            return;
        }

        self.draw_stale_overrides(ui);
        self.draw_conflicts(ui);
        ui.add_space(6.0);
        ui.label(RichText::new("Load order: when two mods change the same file, the one lower in the list wins.").color(look.dim));
        ui.add_space(6.0);
        self.draw_mod_rows(ui);
    }

    fn draw_mod_rows(&mut self, ui: &mut Ui) {
        let look = look(ui);
        let ids: Vec<ProviderId> = self.library.mods.iter().map(|entry| entry.id).collect();
        let count = ids.len();
        let mut changed = false;
        let mut remove: Option<ProviderId> = None;
        let mut move_later: Option<ProviderId> = None;
        let mut move_earlier: Option<ProviderId> = None;

        for (place, id) in ids.into_iter().enumerate() {
            let Some(index) = self.library.mods.iter().position(|entry| entry.id == id) else {
                continue;
            };
            let is_enabled = self.library.mods[index].enabled;
            egui::Frame::new()
                .fill(if is_enabled { look.card } else { look.sunk })
                .stroke(egui::Stroke::new(1.5, if is_enabled { look.accent } else { look.edge }))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}", place + 1)).size(20.0).strong().color(look.dim));
                        ui.add_space(6.0);
                        let (word, colour) = if is_enabled { ("On", Some(Color32::from_rgb(80, 220, 120))) } else { ("Off", None) };
                        if act(ui, look, word, colour).on_hover_text("Turn this mod on or off").clicked() {
                            self.library.mods[index].enabled = !is_enabled;
                            changed = true;
                        }
                        ui.add_space(6.0);
                        let renaming = matches!(&self.rename, Some((rid, _)) if *rid == id);
                        ui.vertical(|ui| {
                            if renaming {
                                if let Some((_, buffer)) = &mut self.rename {
                                    let response = ui.add(egui::TextEdit::singleline(buffer).desired_width(260.0).hint_text("Mod name"));
                                    if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        let name = buffer.trim().to_string();
                                        if !name.is_empty() {
                                            self.library.mods[index].name = name;
                                            changed = true;
                                        }
                                        self.rename = None;
                                    }
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.rename = None;
                                    }
                                }
                            } else {
                                let entry = &self.library.mods[index];
                                let name = ui.add(egui::Label::new(RichText::new(&entry.name).size(17.0).strong().color(look.text)).sense(egui::Sense::click()));
                                if name.on_hover_text("Click to rename").clicked() {
                                    self.rename = Some((id, entry.name.clone()));
                                }
                            }
                            ui.label(RichText::new(self.library.mods[index].summary()).color(look.dim));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if act(ui, look, "Remove", None).on_hover_text("Take this mod out of the list").clicked() {
                                remove = Some(id);
                            }
                            let expanded = self.expanded == Some(id);
                            if act(ui, look, if expanded { "Hide files" } else { "Files" }, None).clicked() {
                                self.expanded = (!expanded).then_some(id);
                            }
                            if place + 1 < count && act(ui, look, "Down", None).on_hover_text("Later in the load order: wins more").clicked() {
                                move_later = Some(id);
                            }
                            if place > 0 && act(ui, look, "Up", None).on_hover_text("Earlier in the load order: wins less").clicked() {
                                move_earlier = Some(id);
                            }
                        });
                    });

                    let entry = &self.library.mods[index];
                    // Always visible: a shipped plugin means its movesets read as the game's.
                    if entry.ships_plugin() {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "Has a compiled plugin ({}). The editor cannot read it, so its moves show as the game's.",
                                entry.manifest.plugins.join(", ")
                            ))
                            .color(Color32::from_rgb(240, 190, 90)),
                        );
                    }
                    if self.expanded != Some(id) {
                        return;
                    }
                    ui.add_space(6.0);
                    if !matches!(entry.detection, library::RootDetection::AsGiven) {
                        ui.label(RichText::new(format!("Mod folder: {}", entry.detection.describe())).color(look.dim));
                    }
                    for (fighter, provision) in &entry.manifest.fighters {
                        let slots = provision.slots.iter().map(|slot| format!("c{slot:02}")).collect::<Vec<_>>().join(", ");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(fighter).strong().color(look.text));
                            ui.label(RichText::new(slots).color(look.dim));
                        });
                    }
                    if entry.manifest.unrecognized > 0 {
                        ui.label(RichText::new(format!("{} files outside the game's folders were left out (readmes, pictures).", entry.manifest.unrecognized)).color(look.dim));
                    }
                    ui.add_space(4.0);
                    let paths: Vec<String> = entry.manifest.paths.iter().cloned().collect();
                    egui::Frame::new().fill(look.sunk).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical().id_salt(("roster_mod_files", id.0)).max_height(200.0).show(ui, |ui| {
                            for path in paths {
                                ui.label(RichText::new(path).monospace().color(look.text));
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        if let Some(id) = move_later {
            self.library.move_later(id);
            changed = true;
        }
        if let Some(id) = move_earlier {
            self.library.move_earlier(id);
            changed = true;
        }
        if let Some(id) = remove {
            self.library.remove(id);
            changed = true;
        }
        if changed {
            self.commit_library_change();
        }
    }

    fn draw_conflicts(&mut self, ui: &mut Ui) {
        let by_fighter = self.library.conflicts_by_fighter();
        let total: usize = by_fighter.values().map(|list| list.len()).sum();
        if total == 0 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("No file conflicts between enabled mods.")
                        .small()
                        .weak(),
                );
            });
            return;
        }
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(240, 200, 120), "⚠");
                    ui.label(
                        RichText::new(format!(
                            "{total} file(s) provided by more than one enabled mod. Later in load order wins."
                        ))
                        .small()
                        .strong(),
                    );
                });
                ui.add_space(4.0);
                for (fighter, conflicts) in by_fighter {
                    let expanded = self.expanded_conflict.as_deref() == Some(fighter.as_str());
                    let winners: Vec<String> = {
                        let mut names: Vec<String> = conflicts
                            .iter()
                            .filter_map(|conflict| conflict.winner())
                            .map(|id| self.library.name_of(id))
                            .collect();
                        names.sort();
                        names.dedup();
                        names
                    };
                    let header = format!(
                        "{fighter}: {} file(s), won by {}",
                        conflicts.len(),
                        winners.join(", ")
                    );
                    let resp = ui.selectable_label(expanded, RichText::new(header).small());
                    if resp.clicked() {
                        self.expanded_conflict = (!expanded).then(|| fighter.clone());
                    }
                    if expanded {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt(("roster_conflicts", fighter.clone()))
                                    .max_height(160.0)
                                    .show(ui, |ui| {
                                        for conflict in &conflicts {
                                            let chain = conflict
                                                .providers
                                                .iter()
                                                .map(|id| self.library.name_of(*id))
                                                .collect::<Vec<_>>()
                                                .join(" → ");
                                            ui.label(
                                                RichText::new(format!("{}  [{}]", conflict.game_path, chain))
                                                    .small()
                                                    .monospace()
                                                    .weak(),
                                            );
                                        }
                                    });
                            });
                    }
                }
            });
    }

    /// Project edits that no longer have an entry behind them.
    ///
    /// Shown in the library panel because disabling a mod is how they usually become stale,
    /// and because the fix — re-enable the mod — is right here.
    fn draw_stale_overrides(&mut self, ui: &mut Ui) {
        if self.index.stale_overrides.is_empty() {
            return;
        }
        ui.add_space(6.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(240, 140, 140), "⚠");
                    ui.label(
                        RichText::new(format!(
                            "{} project edit(s) with no matching character. Kept, not discarded. Re-enable the mod that provided it to restore them.",
                            self.index.stale_overrides.len()
                        ))
                        .small()
                        .strong(),
                    );
                });
                ui.add_space(4.0);
                for stale in &self.index.stale_overrides {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("•").small().weak());
                        ui.label(
                            RichText::new(format!("{}. {}", stale.key, stale.kind.describe()))
                                .small()
                                .weak(),
                        );
                    });
                }
                if ui.small_button("Clear stale edits").on_hover_text("Discard edits that have no character behind them").clicked() {
                    // Remove stale keys from all maps; keep authored list filtered
                    let stale_keys: std::collections::BTreeSet<_> = self.index.stale_overrides.iter().map(|s| &s.key).collect();
                    self.project.order.retain(|k, _| !stale_keys.contains(k));
                    self.project.hidden.retain(|k| !stale_keys.contains(k));
                    self.project.names.retain(|k, _| !stale_keys.contains(k));
                    self.project.name_variants.retain(|k, _| !stale_keys.contains(k));
                    self.project.ui_images.retain(|k, _| !stale_keys.contains(k));
                    self.project.chara_overrides.retain(|k, _| !stale_keys.contains(k));
                    self.project.authored.retain(|e| !stale_keys.contains(&e.key));
                    // Per-costume names are keyed by fighter string, not RosterKey; drop fighters that are now stale.
                    self.project.per_costume_names.retain(|fighter, _| {
                        let key = crate::roster::RosterKey::fighter(fighter);
                        !stale_keys.contains(&key)
                    });
                }
            });
    }

    fn draw_notes(&mut self, ui: &mut Ui) {
        if self.notes.is_empty() {
            return;
        }
        ui.add_space(4.0);
        let failures = self
            .notes
            .iter()
            .filter(|note| note.outcome.is_err())
            .count();
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let ok = self.notes.len() - failures;
                    ui.label(
                        RichText::new(format!("Last import: {ok} succeeded, {failures} failed"))
                            .small()
                            .strong()
                            .color(if failures == 0 {
                                Color32::from_rgb(130, 225, 150)
                            } else {
                                Color32::from_rgb(240, 200, 120)
                            }),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.notes.clear();
                        }
                    });
                });
                ui.add_space(4.0);
                for note in &self.notes {
                    match &note.outcome {
                        Ok(detail) => ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(130, 225, 150), "✔");
                            ui.label(
                                RichText::new(format!("{}. {detail}", note.source))
                                    .small()
                                    .weak(),
                            );
                        }),
                        Err(error) => ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(240, 140, 140), "✘");
                            ui.label(
                                RichText::new(format!("{}. {error}", note.source))
                                    .small()
                                    .weak(),
                            );
                        }),
                    };
                }
            });
        ui.add_space(4.0);
    }

    // ── Import ──────────────────────────────────────────────────────────────

    fn start_import(&mut self, paths: Vec<PathBuf>) {
        if self.import.is_some() {
            self.status = "An import is already running. Wait for it to finish.".into();
            return;
        }
        if paths.is_empty() {
            return;
        }
        let total = paths.len();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.notes.clear();
        std::thread::spawn(move || import_worker(paths, sender));
        self.import = Some(ImportJob {
            receiver,
            done: 0,
            total,
            current: String::new(),
        });
    }

    fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.import else { return };
        let mut finished = false;
        let mut inserted = Vec::new();
        let mut failures = Vec::new();
        loop {
            match job.receiver.try_recv() {
                Ok(ImportMsg::Progress { done, total, name }) => {
                    job.done = done;
                    job.total = total;
                    job.current = name;
                }
                Ok(ImportMsg::Prepared(prepared)) => inserted.push(*prepared),
                Ok(ImportMsg::Failed { source, error }) => failures.push((source, error)),
                Ok(ImportMsg::Finished) => finished = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }

        let progressed = !inserted.is_empty() || !failures.is_empty();
        for prepared in inserted {
            let source = prepared.source.origin_path().display().to_string();
            let detail = format!(
                "{} file(s), {} fighter(s); {}",
                prepared.manifest.paths.len(),
                prepared.manifest.fighters.len(),
                prepared.detection.describe()
            );
            self.library.insert(prepared);
            self.notes.push(ImportNote {
                source,
                outcome: Ok(detail),
            });
        }
        for (source, error) in failures {
            self.notes.push(ImportNote {
                source,
                outcome: Err(error),
            });
        }
        if progressed {
            self.commit_library_change();
        }
        if finished {
            self.import = None;
            let failed = self
                .notes
                .iter()
                .filter(|note| note.outcome.is_err())
                .count();
            self.status = format!(
                "Imported {} mod(s), {failed} failed.",
                self.notes.len() - failed
            );
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    pub fn request_rescan(&mut self) {
        self.rescan();
    }

    fn rescan(&mut self) {
        let mut rescanned = 0;
        let mut lost = Vec::new();
        for entry in &mut self.library.mods {
            match library::scan_manifest(&entry.root) {
                Ok(manifest) => {
                    entry.manifest = manifest;
                    rescanned += 1;
                }
                Err(error) => lost.push(format!("{}: {error}", entry.name)),
            }
        }
        self.commit_library_change();
        self.status = if lost.is_empty() {
            format!("Rescanned {rescanned} mod(s).")
        } else {
            format!("Rescanned {rescanned}; failed: {}", lost.join("; "))
        };
    }

    /// Persist the library and tell `app.rs` its fighter index is stale.
    ///
    /// Every mutation routes through here. A path that changed the library without setting
    /// the dirty flag would leave the fighter list showing a mod the user just disabled.
    fn commit_library_change(&mut self) {
        library::save(&self.library);
        self.library_dirty = true;
        // A newly enabled mod may now provide the roster database, or win a portrait an
        // earlier root was providing. Both are cached, so both have to be dropped here.
        self.css.invalidate();
        self.traits.invalidate();
    }
}

/// Summary of roster edits for the export tree and header badges.
#[derive(Debug, Clone, Default)]
pub struct RosterExportSummary {
    pub order: usize,
    pub hidden: usize,
    pub names: usize,
    pub authored: usize,
    pub params: usize,
}

/// One roster edit kind the Edit Log can revert wholesale. New characters
/// are listed but never reverted from the log — removing a character
/// deserves its own confirmation where its files live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterLogClear {
    Positions,
    Hidden,
    DisplayNames,
    NameVariants,
    CostumeNames,
    Images,
    RowPatches,
}

/// Prepare each import on a worker thread.
///
/// Extraction and the manifest walk are both filesystem-bound and a large mod archive takes
/// seconds; doing this inline froze the window for the whole batch. Each item reports its own
/// outcome, so one corrupt archive costs that archive and nothing else.
fn import_worker(paths: Vec<PathBuf>, sender: Sender<ImportMsg>) {
    let total = paths.len();
    for (index, path) in paths.into_iter().enumerate() {
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        if sender
            .send(ImportMsg::Progress {
                done: index,
                total,
                name: label.clone(),
            })
            .is_err()
        {
            return;
        }

        let source = if super::archive::is_archive(&path) {
            match super::archive::extract(&path) {
                Ok(extracted) => ModSource::Archive {
                    archive: path.clone(),
                    extracted,
                },
                Err(error) => {
                    let _ = sender.send(ImportMsg::Failed {
                        source: label,
                        error: format!("{error:#}"),
                    });
                    continue;
                }
            }
        } else {
            ModSource::Folder(path.clone())
        };

        match library::prepare(source, None) {
            Ok(prepared) => {
                let _ = sender.send(ImportMsg::Prepared(Box::new(prepared)));
            }
            Err(error) => {
                let _ = sender.send(ImportMsg::Failed {
                    source: label,
                    error: format!("{error:#}"),
                });
            }
        }
    }
    let _ = sender.send(ImportMsg::Finished);
}

#[cfg(test)]
mod gimmick_root_tests {
    use super::*;

    fn add_mod(window: &mut RosterWindow, root: &std::path::Path, enabled: bool) {
        window.library.insert(library::PreparedMod {
            name: root.display().to_string(),
            source: ModSource::Folder(root.to_path_buf()),
            root: root.to_path_buf(),
            detection: library::RootDetection::AsGiven,
            manifest: Default::default(),
        });
        window.library.mods.last_mut().unwrap().enabled = enabled;
    }

    #[test]
    fn personal_baselines_follow_enabled_mod_priority_and_deduplicate() {
        let mut window = RosterWindow::default();
        let data = PathBuf::from("base");
        window.set_export_root(Some(PathBuf::from("dump")));
        add_mod(&mut window, std::path::Path::new("early"), true);
        add_mod(&mut window, std::path::Path::new("late"), true);
        add_mod(&mut window, std::path::Path::new("disabled"), false);
        add_mod(&mut window, &data, true);
        assert_eq!(
            window.gimmick_roots(Some(&data)),
            ["base", "late", "early", "dump"].map(PathBuf::from)
        );
        assert_eq!(
            window.content_roots(Some(&data)),
            ["base", "dump", "early", "late"].map(PathBuf::from)
        );
    }

    #[test]
    fn personal_lookup_falls_back_without_writing_source_files() {
        let fixture = tempfile::tempdir().unwrap();
        let base = fixture.path().join("base");
        let early = fixture.path().join("early");
        let late = fixture.path().join("late");
        let relative = "fighter/mario/param/vl.prc";
        for (root, bytes) in [
            (&base, b"base".as_slice()),
            (&early, b"early".as_slice()),
            (&late, b"late".as_slice()),
        ] {
            std::fs::create_dir_all(root.join("fighter/mario/param")).unwrap();
            std::fs::write(root.join(relative), bytes).unwrap();
        }
        let mut window = RosterWindow::default();
        add_mod(&mut window, &early, true);
        add_mod(&mut window, &late, true);
        let locate = |window: &RosterWindow| {
            crate::gimmicks::params::locate_personal(&window.gimmick_roots(Some(&base)), "mario")
                .unwrap()
        };
        assert_eq!(locate(&window), late.join(relative));
        window.library.mods[1].enabled = false;
        assert_eq!(locate(&window), early.join(relative));
        window.library.mods[0].enabled = false;
        assert_eq!(locate(&window), base.join(relative));
        for (root, bytes) in [
            (&base, b"base".as_slice()),
            (&early, b"early".as_slice()),
            (&late, b"late".as_slice()),
        ] {
            assert_eq!(std::fs::read(root.join(relative)).unwrap(), bytes);
        }
    }

    #[test]
    fn personal_export_patches_the_winning_mod_and_preserves_its_other_values() {
        use crate::gimmicks::{params, paths::NestedPath};
        use crate::mod_project::ParamValue;
        let fixture = tempfile::tempdir().unwrap();
        let base = fixture.path().join("base");
        let custom = fixture.path().join("custom");
        let output = fixture.path().join("output");
        let relative = "fighter/mario/param/vl.prc";
        for (root, untouched) in [(&base, 1.0), (&custom, 7.0)] {
            std::fs::create_dir_all(root.join("fighter/mario/param")).unwrap();
            let tree = prc::ParamStruct(vec![
                (
                    prc::hash40::Hash40(hash40::hash40("charge_rate").0),
                    prc::ParamKind::Float(2.0),
                ),
                (
                    prc::hash40::Hash40(hash40::hash40("untouched").0),
                    prc::ParamKind::Float(untouched),
                ),
            ]);
            params::save_personal(&tree, &root.join(relative), "mario").unwrap();
        }
        let source_before = std::fs::read(custom.join(relative)).unwrap();
        let mut window = RosterWindow {
            initialized: true,
            ..Default::default()
        };
        add_mod(&mut window, &custom, true);
        params::record_personal_edit(
            window.params.entry("mario".into()).or_default(),
            "mario",
            &NestedPath::parse("charge_rate").unwrap(),
            ParamValue::Float(4.0),
            Some(ParamValue::Float(2.0)),
        )
        .unwrap();
        window
            .export_into(&output, &RosterMod::default(), &[], Some(&base))
            .unwrap();
        let exported = params::open_personal(&output.join(relative), "mario").unwrap();
        assert_eq!(
            params::read_leaf(
                &exported,
                "mario",
                &NestedPath::parse("charge_rate").unwrap()
            )
            .unwrap(),
            ParamValue::Float(4.0)
        );
        assert_eq!(
            params::read_leaf(&exported, "mario", &NestedPath::parse("untouched").unwrap())
                .unwrap(),
            ParamValue::Float(7.0)
        );
        assert_eq!(std::fs::read(custom.join(relative)).unwrap(), source_before);
    }
}
