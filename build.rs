//! En Windows incrusta el ícono y los datos del programa en el .exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Notas")
            .set("FileDescription", "Notas")
            .set("CompanyName", "JP Reyes")
            .set("LegalCopyright", "MIT · github.com/jpreyes/nodex-notes");
        if let Err(e) = res.compile() {
            println!("cargo:warning=No se pudo incrustar el ícono: {e}");
        }
    }
}
