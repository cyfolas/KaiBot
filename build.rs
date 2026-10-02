//! `sqlx::migrate!` встраивает миграции при компиляции, но сам не отслеживает их изменения:
//! без этой строки новая миграция не попадёт в бинарник до `cargo clean`.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
