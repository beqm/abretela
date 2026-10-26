fn main() {
    // o ícone do .exe e da janela é embutido na compilação; sem isso o Cargo
    // reaproveita o recurso antigo quando só o ícone muda
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
