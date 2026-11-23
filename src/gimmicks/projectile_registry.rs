/// Resolve a reviewed weapon Work field to its exact article kind.
pub fn projectile_work(
    fighter: &str,
    symbol: &str,
    kind: IdentifierKind,
) -> Option<(&'static RegistryEntry, &'static RegistryEntry)> {
    if !symbol.starts_with("WEAPON_")
        || !matches!(
            kind,
            IdentifierKind::WorkFloat | IdentifierKind::WorkInt | IdentifierKind::WorkFlag
        )
    {
        return None;
    }
    let field = ENTRIES
        .iter()
        .find(|entry| entry.fighter == fighter && entry.symbol == symbol && entry.kind == kind)?;
    if field.group.is_empty()
        || !symbol.starts_with(&format!(
            "WEAPON_{}_{}_",
            fighter.to_ascii_uppercase(),
            field.group.to_ascii_uppercase()
        ))
    {
        return None;
    }
    weapon_kind(fighter, field.group)?;
    let article = ENTRIES
        .iter()
        .filter(|entry| entry.fighter == fighter && entry.kind == IdentifierKind::Article)
        .find(|entry| {
            entry
                .symbol
                .rsplit_once("_GENERATE_ARTICLE_")
                .is_some_and(|(_, group)| group.eq_ignore_ascii_case(field.group))
        })?;
    Some((field, article))
}
