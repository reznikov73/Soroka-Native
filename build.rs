//! Манифест приложения (TASK-N04.3) — корень мерцания сплиттеров.
//!
//! exe без манифеста получает от загрузчика comctl32 **5.82**, а
//! расширенные стили двойной буферизации (LVS_EX_DOUBLEBUFFER,
//! TVS_EX_DOUBLEBUFFER, HDS_DOUBLEBUFFER) — фичи comctl32 6 (Vista+):
//! без манифеста они молча не работали, дерево и список стирались и
//! перерисовывались на каждый шаг мыши.
//!
//! Манифест (comctl32 v6 + PerMonitorV2) встраивается link.exe
//! (/MANIFEST:EMBED /MANIFESTINPUT) — только для MSVC-таргетов, там
//! живая сборка AutoClaw. GNU-кросс-сборка в песочнице линкуется
//! rust-lld без манифеста: она нужна для check/clippy/test --no-run и
//! пользователю не доставляется.
fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(&dir).join("app.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
