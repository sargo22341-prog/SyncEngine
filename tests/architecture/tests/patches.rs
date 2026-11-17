//! M10 — Étant donné une entrée `[patch.crates-io]`, quand `cargo update`
//! choisit une autre version que la copie corrigée, alors la CI échoue au
//! lieu de livrer silencieusement le code amont non corrigé.

#[test]
fn every_vendored_patch_is_used() -> Result<(), Box<dyn std::error::Error>> {
    let lock = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"),
    )?;
    assert!(
        !lock.contains("[[patch.unused]]"),
        "Cargo.lock porte un [[patch.unused]] : un correctif de vendor/ n'est plus utilisé \
         (réaligner la version, l'épingler ou retirer le correctif)"
    );
    Ok(())
}
