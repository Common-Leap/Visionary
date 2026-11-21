include!("../../plugins/slight_replica/src/slight/systems/article_live.rs");

use std::collections::HashMap as TestMap;

fn snapshot_map(entries: &[ObjectSnapshot]) -> TestMap<u32, ObjectSnapshot> {
    entries.iter().copied().map(|s| (s.boid, s)).collect()
}

fn mock_agent(kind: i32) -> Option<String> {
    match kind {
        0 => Some("mario_fireball".into()),
        0x42 => Some("pacman_trampoline".into()),
        0x43 => Some("pacman_firehydrant".into()),
        _ => None,
    }
}

#[test]
fn foreign_article_rejected() {
    let objects = snapshot_map(&[
        // Selected fighter: Mario (kind 0, costume 0)
        ObjectSnapshot {
            boid: 0x2000_0001,
            category: 0,
            kind: 0,
            costume: 0,
            parent_boid: None,
        },
        // Foreign fighter: Samus (kind 4, costume 0)
        ObjectSnapshot {
            boid: 0x2000_0002,
            category: 0,
            kind: 4,
            costume: 0,
            parent_boid: None,
        },
        // Weapon owned by Samus
        ObjectSnapshot {
            boid: 0x3000_0010,
            category: 1,
            kind: 0,
            costume: -1,
            parent_boid: Some(0x2000_0002),
        },
        // Slot-scoped fighter: Pac-Man vanilla (c00) vs Shy Guy (c40)
        ObjectSnapshot {
            boid: 0x2000_0003,
            category: 0,
            kind: 55,
            costume: 0,
            parent_boid: None,
        },
        ObjectSnapshot {
            boid: 0x3000_0020,
            category: 1,
            kind: 0x42,
            costume: -1,
            parent_boid: Some(0x2000_0003),
        },
    ]);

    let selected_mario = SelectedFighter {
        kind: 0,
        costumes: vec![],
    };
    assert_eq!(
        resolve_selected_owner_article(
            0x3000_0010,
            Some(&selected_mario),
            |id| objects.get(&id).copied(),
            mock_agent,
        ),
        None,
        "an article owned by another fighter must be rejected"
    );

    // Slot-scoped selection (Shy Guy on Pac-Man c40..=c47) rejects vanilla Pac-Man c00's article.
    let selected_shy_guy = SelectedFighter {
        kind: 55,
        costumes: vec![40, 41, 42, 43, 44, 45, 46, 47],
    };
    assert_eq!(
        resolve_selected_owner_article(
            0x3000_0020,
            Some(&selected_shy_guy),
            |id| objects.get(&id).copied(),
            mock_agent,
        ),
        None,
        "another costume slot of the same fighter must not match a slot-scoped character"
    );

    // Unselected fighter rejects everything.
    assert_eq!(
        resolve_selected_owner_article(
            0x3000_0010,
            None,
            |id| objects.get(&id).copied(),
            mock_agent,
        ),
        None
    );
}

#[test]
fn nested_owner_accepted() {
    let objects = snapshot_map(&[
        // Shy Guy on Pac-Man c40
        ObjectSnapshot {
            boid: 0x2000_0005,
            category: 0,
            kind: 55,
            costume: 40,
            parent_boid: None,
        },
        // Parent article: trampoline owned by Shy Guy
        ObjectSnapshot {
            boid: 0x3000_0030,
            category: 1,
            kind: 0x42,
            costume: -1,
            parent_boid: Some(0x2000_0005),
        },
        // Nested child article: fireball owned by trampoline
        ObjectSnapshot {
            boid: 0x3000_0031,
            category: 1,
            kind: 0,
            costume: -1,
            parent_boid: Some(0x3000_0030),
        },
        // Cycle guard: weapon pointing to itself
        ObjectSnapshot {
            boid: 0x3000_0099,
            category: 1,
            kind: 0,
            costume: -1,
            parent_boid: Some(0x3000_0099),
        },
    ]);

    let selected_shy_guy = SelectedFighter {
        kind: 55,
        costumes: vec![40, 41, 42, 43, 44, 45, 46, 47],
    };
    let key = resolve_selected_owner_article(
        0x3000_0031,
        Some(&selected_shy_guy),
        |id| objects.get(&id).copied(),
        mock_agent,
    );
    assert_eq!(
        key,
        Some(ArticleKey {
            owner_kind: 55,
            article_kind: 0,
            agent: "mario_fireball".into(),
        })
    );

    // A self-cycle never hangs and returns None.
    assert_eq!(
        resolve_selected_owner_article(
            0x3000_0099,
            Some(&selected_shy_guy),
            |id| objects.get(&id).copied(),
            mock_agent,
        ),
        None
    );
}

#[test]
fn fighter_capture_unaffected() {
    // Fighter kind 0 (Mario) and article kind 0 (numerically equal — category-local collision).
    let mut tables = CaptureRoutingTables::default();
    let motion = 0x1234_5678_u64;

    // Record a fighter line for fighter kind 0.
    tables.record_fighter(0, motion, "ATTACK(fighter)".into());

    // Record an article line whose article_kind is ALSO 0 (same numeric value as Mario!).
    let key = ArticleKey {
        owner_kind: 0,
        article_kind: 0,
        agent: "mario_fireball".into(),
    };
    tables.record_article(&key, motion, "ATTACK(fireball)".into());

    // Fighter table must contain ONLY the fighter line, never the article line.
    assert_eq!(
        tables.fighter_lines(0, motion),
        &["ATTACK(fighter)".to_string()]
    );
    // Article table is keyed by (agent, motion) and holds ONLY the article line.
    assert_eq!(
        tables.article_lines("mario_fireball", motion),
        &["ATTACK(fireball)".to_string()]
    );
    assert!(tables.article_lines("samus_cshot", motion).is_empty());
}
