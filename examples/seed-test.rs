//! Creates only a synthetic profile in an explicitly supplied isolated test directory.
use thodtalk::storage::{Profile, Store};
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Укажите пустой тестовый каталог"))?;
    let store = Store::open(path.into())?;
    anyhow::ensure!(
        store.load::<Profile>("profile")?.is_none(),
        "Каталог уже содержит профиль"
    );
    store.save(
        "profile",
        &Profile {
            name: "Тестовый участник".into(),
            network_allowed: true,
            ..Default::default()
        },
    )
}
