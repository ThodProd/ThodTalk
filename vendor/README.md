iroh 1.4.1 из crates.io. Единственное изменение: удалён `cdylib` из
`crate-type` в Cargo.toml. ThodTalk статически использует Rust-библиотеку;
неиспользуемая DLL превышает лимит экспортов PE при сборке Windows GNU.
Исходники и лицензии upstream сохранены.
