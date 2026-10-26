// Incrusta el ícono en el .exe (Explorador, barra de tareas, bandeja).
fn main() {
    println!("cargo:rerun-if-changed=pega.rc");
    println!("cargo:rerun-if-changed=assets/pega.ico");
    embed_resource::compile("pega.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
