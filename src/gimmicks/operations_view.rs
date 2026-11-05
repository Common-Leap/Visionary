//! Compact live controls for the constrained F1.5 operation boundary.

use egui::{RichText, Ui};

use super::registry::{self, IdentifierKind, RegistryEntry};
use crate::game_link::{GameLink, LinkStatus, LiveGimmickAgent};

#[derive(Default)]
pub struct OperationsView {
    fighter: String,
    fighter_boid: Option<u32>,
    weapon_boid: Option<u32>,
    article_symbol: String,
    status_symbol: String,
    message: String,
}

impl OperationsView {
    pub fn ui(&mut self, ui: &mut Ui, fighter: &str, link: &GameLink) {
        if fighter.is_empty() {
            return;
        }
        // Live object choices arrive over the same reconnecting link as the
        // operations themselves. Start it when this fighter-scoped panel is
        // available so users do not need a failed manual request to populate it.
        link.ensure_started();
        if self.fighter != fighter {
            self.fighter = fighter.to_string();
            self.fighter_boid = None;
            self.weapon_boid = None;
            self.article_symbol = registry::entries_for(fighter, IdentifierKind::Article)
                .next()
                .map_or_else(String::new, |entry| entry.symbol.to_string());
            self.status_symbol = supported_statuses(fighter)
                .next()
                .map_or_else(String::new, |entry| entry.symbol.to_string());
            self.message.clear();
        }
        let live = link.gimmick_agents();
        let fighters = live_fighters(&live, fighter);
        reconcile_selection(&mut self.fighter_boid, &fighters);

        egui::CollapsingHeader::new("Live operations")
            .id_salt("gimmick_live_operations")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Advanced game-thread requests for already-live objects. Object IDs are \
                         runtime targets, not saved project data; the plugin rechecks ownership, \
                         fighter kind, weapon kind, and every symbolic identifier.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("Fighter object");
                    if object_picker(
                        ui,
                        "gimmick_live_fighter_object",
                        &fighters,
                        &mut self.fighter_boid,
                        true,
                        fighter_picker_placeholder(link, fighter),
                    ) {
                        self.weapon_boid = None;
                    }
                    ui.label(
                        RichText::new(link_summary(link, fighter, fighters.len()))
                            .small()
                            .weak(),
                    );
                });
                if link.status() == LinkStatus::Disconnected {
                    if let Some(error) = link.last_error() {
                        ui.label(RichText::new("Connection details").small().weak())
                            .on_hover_text(error);
                    }
                } else if let Some(error) = link.gimmick_agents_error() {
                    ui.label(RichText::new("Roster details").small().weak())
                        .on_hover_text(error);
                }

                let articles =
                    registry::entries_for(fighter, IdentifierKind::Article).collect::<Vec<_>>();
                if !articles.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        symbol_picker(
                            ui,
                            "gimmick_live_article",
                            &articles,
                            &mut self.article_symbol,
                        );
                        if ui
                            .add_enabled(
                                self.fighter_boid.is_some(),
                                egui::Button::new("Generate").small(),
                            )
                            .clicked()
                        {
                            self.run_article(link, fighter, true);
                        }
                        if ui
                            .add_enabled(
                                self.fighter_boid.is_some(),
                                egui::Button::new("Remove").small(),
                            )
                            .clicked()
                        {
                            self.run_article(link, fighter, false);
                        }
                    });
                }

                let statuses = supported_statuses(fighter).collect::<Vec<_>>();
                if !statuses.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Weapon status");
                        if symbol_picker(
                            ui,
                            "gimmick_live_weapon_status",
                            &statuses,
                            &mut self.status_symbol,
                        ) {
                            self.weapon_boid = None;
                        }
                    });
                    let weapon_kind = statuses
                        .iter()
                        .find(|entry| entry.symbol == self.status_symbol)
                        .and_then(|entry| registry::weapon_kind(fighter, entry.group))
                        .map(|entry| entry.value);
                    let weapons = live_weapons(&live, self.fighter_boid, weapon_kind);
                    reconcile_selection(&mut self.weapon_boid, &weapons);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Weapon object");
                        object_picker(
                            ui,
                            "gimmick_live_weapon_object",
                            &weapons,
                            &mut self.weapon_boid,
                            false,
                            weapon_picker_placeholder(link, self.fighter_boid, weapon_kind),
                        );
                        if ui
                            .add_enabled(
                                self.fighter_boid.is_some() && self.weapon_boid.is_some(),
                                egui::Button::new("Request status").small(),
                            )
                            .clicked()
                        {
                            self.run_status(link, fighter);
                        }
                    });
                }

                if articles.is_empty() && statuses.is_empty() {
                    ui.label(
                        RichText::new("No supported live operations for this fighter.").small(),
                    );
                }
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).small());
                }
            });
    }

    fn run_article(&mut self, link: &GameLink, fighter: &str, generate: bool) {
        let result = self
            .fighter_boid
            .ok_or_else(|| {
                "Choose a live fighter from the selector before sending the request.".to_string()
            })
            .and_then(|boid| {
                link.ensure_started();
                if generate {
                    link.generate_gimmick_article(fighter, boid, &self.article_symbol)
                } else {
                    link.remove_gimmick_article(fighter, boid, &self.article_symbol)
                }
                .map_err(|error| format!("Request refused: {error}"))
            });
        self.message = result.map_or_else(
            |error| error,
            |()| {
                format!(
                    "{} request queued; game-side acceptance or rejection is written to sd:/slight/diag.txt.",
                    if generate { "Generate" } else { "Remove" }
                )
            },
        );
    }

    fn run_status(&mut self, link: &GameLink, fighter: &str) {
        let result = self
            .fighter_boid
            .ok_or_else(|| {
                "Choose a live fighter from the selector before sending the request.".to_string()
            })
            .and_then(|fighter_boid| {
                self.weapon_boid
                    .ok_or_else(|| {
                        "Choose a matching live weapon from the selector first.".to_string()
                    })
                    .and_then(|weapon_boid| {
                        link.ensure_started();
                        link.request_gimmick_weapon_status(
                            fighter,
                            fighter_boid,
                            weapon_boid,
                            &self.status_symbol,
                        )
                        .map_err(|error| format!("Request refused: {error}"))
                    })
            });
        self.message = result.map_or_else(
            |error| error,
            |()| {
                "Status request queued; game-side acceptance or rejection is written to sd:/slight/diag.txt."
                    .to_string()
            },
        );
    }
}

fn supported_statuses(fighter: &str) -> impl Iterator<Item = &'static RegistryEntry> + '_ {
    registry::entries_for(fighter, IdentifierKind::WeaponStatus)
        .filter(move |entry| registry::weapon_kind(fighter, entry.group).is_some())
}

fn live_fighters<'a>(agents: &'a [LiveGimmickAgent], fighter: &str) -> Vec<&'a LiveGimmickAgent> {
    agents
        .iter()
        .filter(|agent| agent.category == 0 && agent.fighter.as_deref() == Some(fighter))
        .collect()
}

fn live_weapons(
    agents: &[LiveGimmickAgent],
    fighter_boid: Option<u32>,
    weapon_kind: Option<u32>,
) -> Vec<&LiveGimmickAgent> {
    let (Some(fighter_boid), Some(weapon_kind)) = (fighter_boid, weapon_kind) else {
        return Vec::new();
    };
    agents
        .iter()
        .filter(|agent| {
            agent.category == 1
                && agent.kind >= 0
                && agent.kind as u32 == weapon_kind
                && agent.owner_boid == Some(fighter_boid)
        })
        .collect()
}

fn object_picker(
    ui: &mut Ui,
    id: &'static str,
    agents: &[&LiveGimmickAgent],
    selected: &mut Option<u32>,
    show_player: bool,
    empty_text: String,
) -> bool {
    let selected_text = selected
        .and_then(|boid| agents.iter().find(|agent| agent.boid == boid).copied())
        .map(|agent| object_label(agent, show_player))
        .unwrap_or(empty_text);
    let before = *selected;
    ui.add_enabled_ui(!agents.is_empty(), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for agent in agents {
                    ui.selectable_value(
                        selected,
                        Some(agent.boid),
                        object_label(agent, show_player),
                    );
                }
            });
    });
    *selected != before
}

fn object_label(agent: &LiveGimmickAgent, show_player: bool) -> String {
    if show_player && agent.entry_id >= 0 {
        format!("P{} · {:#010x}", agent.entry_id + 1, agent.boid)
    } else {
        format!("{:#010x}", agent.boid)
    }
}

fn symbol_picker(
    ui: &mut Ui,
    id: &'static str,
    entries: &[&RegistryEntry],
    selected: &mut String,
) -> bool {
    let before = selected.clone();
    egui::ComboBox::from_id_salt(id)
        .selected_text(compact_symbol(selected))
        .show_ui(ui, |ui| {
            for entry in entries {
                ui.selectable_value(
                    selected,
                    entry.symbol.to_string(),
                    compact_symbol(entry.symbol),
                )
                .on_hover_text(entry.symbol);
            }
        });
    *selected != before
}

fn compact_symbol(symbol: &str) -> &str {
    symbol
        .split_once("_GENERATE_ARTICLE_")
        .or_else(|| symbol.split_once("_STATUS_KIND_"))
        .map_or(symbol, |(_, suffix)| suffix)
}

fn reconcile_selection(selected: &mut Option<u32>, agents: &[&LiveGimmickAgent]) {
    if selected.is_some_and(|boid| agents.iter().all(|agent| agent.boid != boid)) {
        *selected = None;
    }
    if selected.is_none() && agents.len() == 1 {
        *selected = Some(agents[0].boid);
    }
}

fn link_summary(link: &GameLink, fighter: &str, fighter_count: usize) -> String {
    match link.status() {
        LinkStatus::Disconnected => "Game offline".to_string(),
        LinkStatus::Connecting => "Connecting to game…".to_string(),
        LinkStatus::Connected
            if !link.has_gimmick_agents_snapshot() && link.gimmick_agents_error().is_some() =>
        {
            "Live roster rejected; see details".to_string()
        }
        LinkStatus::Connected if !link.has_gimmick_agents_snapshot() => {
            "Connected; waiting for the live roster…".to_string()
        }
        LinkStatus::Connected if fighter_count == 0 => format!("No live {fighter} fighter"),
        LinkStatus::Connected => format!("{fighter_count} live {fighter} target(s)"),
    }
}

fn fighter_picker_placeholder(link: &GameLink, fighter: &str) -> String {
    match link.status() {
        LinkStatus::Disconnected => "Game offline".to_string(),
        LinkStatus::Connecting => "Connecting…".to_string(),
        LinkStatus::Connected
            if !link.has_gimmick_agents_snapshot() && link.gimmick_agents_error().is_some() =>
        {
            "Live roster rejected".to_string()
        }
        LinkStatus::Connected if !link.has_gimmick_agents_snapshot() => {
            "Waiting for live roster…".to_string()
        }
        LinkStatus::Connected => format!("No live {fighter} fighter"),
    }
}

fn weapon_picker_placeholder(
    link: &GameLink,
    fighter_boid: Option<u32>,
    weapon_kind: Option<u32>,
) -> String {
    if link.status() != LinkStatus::Connected {
        return "Game offline".to_string();
    }
    if !link.has_gimmick_agents_snapshot() {
        return "Waiting for live roster…".to_string();
    }
    if fighter_boid.is_none() {
        return "Choose a fighter first".to_string();
    }
    if weapon_kind.is_none() {
        return "Unsupported weapon kind".to_string();
    }
    "No matching live weapon".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_selection_is_live_only_and_auto_selects_one_candidate() {
        let first = live_agent(42, 0, 29, None, Some("cloud"));
        let second = live_agent(43, 0, 29, None, Some("cloud"));
        let mut selected = None;
        reconcile_selection(&mut selected, &[&first]);
        assert_eq!(selected, Some(42));
        reconcile_selection(&mut selected, &[&second]);
        assert_eq!(selected, Some(43));
        reconcile_selection(&mut selected, &[]);
        assert_eq!(selected, None);
    }

    #[test]
    fn status_picker_exposes_only_groups_with_a_weapon_kind_guard() {
        let statuses = supported_statuses("cloud").collect::<Vec<_>>();
        assert!(!statuses.is_empty());
        assert!(statuses
            .iter()
            .all(|entry| registry::weapon_kind("cloud", entry.group).is_some()));
        assert!(supported_statuses("mario")
            .all(|entry| { registry::weapon_kind("mario", entry.group).is_some() }));
    }

    #[test]
    fn picker_labels_hide_only_the_redundant_registry_prefix() {
        assert_eq!(
            compact_symbol("FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"),
            "WAVE"
        );
        assert_eq!(
            compact_symbol("WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR"),
            "REGULAR"
        );
        assert_eq!(compact_symbol("UNEXPECTED"), "UNEXPECTED");
    }

    fn live_agent(
        boid: u32,
        category: i32,
        kind: i32,
        owner_boid: Option<u32>,
        fighter: Option<&str>,
    ) -> LiveGimmickAgent {
        LiveGimmickAgent {
            boid,
            category,
            kind,
            entry_id: if category == 0 { 0 } else { -1 },
            owner_boid,
            fighter: fighter.map(str::to_string),
        }
    }

    #[test]
    fn live_candidates_require_exact_fighter_kind_and_weapon_owner() {
        // The game's first fighter is legitimately battle-object id zero.
        let fighter_boid = 0;
        let cloud_wave_kind = registry::weapon_kind("cloud", "wave").unwrap().value;
        let agents = vec![
            live_agent(fighter_boid, 0, 0x3E, None, Some("cloud")),
            live_agent(0x2000_0002, 0, 0x00, None, Some("mario")),
            live_agent(
                0x1000_0001,
                1,
                cloud_wave_kind as i32,
                Some(fighter_boid),
                None,
            ),
            live_agent(
                0x1000_0002,
                1,
                cloud_wave_kind as i32,
                Some(0x2000_0002),
                None,
            ),
        ];
        assert_eq!(live_fighters(&agents, "cloud")[0].boid, fighter_boid);
        assert_eq!(
            live_weapons(&agents, Some(fighter_boid), Some(cloud_wave_kind))[0].boid,
            0x1000_0001
        );
        assert!(live_weapons(&agents, Some(fighter_boid), Some(0xDEAD)).is_empty());
    }
}
