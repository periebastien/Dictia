//! Intègre l'icône et les informations de version dans `dictia.exe`.

fn main() {
    println!("cargo:rerun-if-changed=assets/dictia.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    // Identifiant 1 : icône de l'exécutable, aussi chargée par la fenêtre.
    res.set_icon("assets/dictia.ico")
        .set("ProductName", "Dictia")
        .set("FileDescription", "Dictia — dictée vocale")
        .set("LegalCopyright", "Dictia");
    if let Err(e) = res.compile() {
        panic!("compilation des ressources Windows impossible : {e}");
    }
}
