//! Манифест приложения (TASK-N04.3) — корень мерцания сплиттеров.
//!
//! exe без манифеста получает от загрузчика comctl32 **5.82**, а
//! расширенные стили двойной буферизации (LVS_EX_DOUBLEBUFFER,
//! TVS_EX_DOUBLEBUFFER, HDS_DOUBLEBUFFER) — фичи comctl32 6 (Vista+):
//! без манифеста они молча не работают, дерево и список стирались и
//! перерисовывались на каждый шаг мыши.
//!
//! MSVC-таргет (живая сборка AutoClaw): манифест встраивает link.exe
//! (/MANIFEST:EMBED /MANIFESTINPUT) — подтверждено на артефакте v0.3.10+
//! (Common-Controls внутри exe).
//!
//! GNU-таргет (раунд 3, TASK-N04): манифест компилирует windres в COFF-объект
//! с секцией .rsrc — RT_MANIFEST оказывается в ЛЮБОЙ Windows-сборке, а не
//! только MSVC. Нет windres в PATH (песочница без кросс-binutils) —
//! предупреждение и сборка без манифеста: gnu-сборка служебная (check/
//! clippy/тесты логики), пользователю не доставляется.
fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    let target = std::env::var("TARGET").unwrap_or_default();
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let manifest = std::path::Path::new(&dir).join("app.manifest");
    if target.contains("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    } else if target.contains("windows-gnu") {
        embed_gnu_manifest(&target, &manifest);
    }
}

/// gnu-таргет: `1 24 "app.manifest"` → windres → COFF-объект → линковщику.
/// Префикс windres выводится из архитектуры таргета (x86_64-w64-mingw32-...).
fn embed_gnu_manifest(target: &str, manifest: &std::path::Path) {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let rc = out_dir.join("app.rc");
    let obj = out_dir.join("app_manifest.o");
    // Прямые слэши: так путь принимают и POSIX-хост, и windres на Windows.
    let manifest_path = manifest.to_string_lossy().replace('\\', "/");
    if std::fs::write(&rc, format!("1 24 \"{manifest_path}\"\n")).is_err() {
        println!("cargo:warning=app.rc не записан: gnu-сборка без манифеста");
        return;
    }
    let arch = target.split('-').next().unwrap_or("x86_64");
    let windres = format!("{arch}-w64-mingw32-windres");
    let ok = std::process::Command::new(&windres)
        .arg("--input")
        .arg(&rc)
        .arg("--output")
        .arg(&obj)
        .arg("--output-format=coff")
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg={}", obj.display());
    } else {
        println!(
            "cargo:warning={windres} не найден: gnu-сборка без манифеста \
             comctl32 v6 (служебная, не доставляется)"
        );
    }
}
