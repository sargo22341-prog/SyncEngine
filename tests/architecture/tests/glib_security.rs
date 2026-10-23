//! Run under Linux in release mode: the original out-pointer UB appears with
//! optimization. The dependency is the same patched GLib used by GTK/Tauri.
#![cfg(target_os = "linux")]
use glib::variant::ToVariant;

#[test]
fn variant_string_iterator_all_directions_survive_optimization()
-> Result<(), Box<dyn std::error::Error>> {
    let strings = ["alpha", "été", "omega"];
    for _ in 0..1000 {
        let variant = strings.to_variant();
        assert_eq!(variant.array_iter_str()?.collect::<Vec<_>>(), strings);
        assert_eq!(
            variant.array_iter_str()?.rev().collect::<Vec<_>>(),
            ["omega", "été", "alpha"]
        );
        assert_eq!(variant.array_iter_str()?.nth(1), Some("été"));
        assert_eq!(variant.array_iter_str()?.nth_back(1), Some("été"));
        assert_eq!(variant.array_iter_str()?.last(), Some("omega"));
    }
    Ok(())
}
