fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "RustSnip");
        res.set("FileDescription", "RustSnip - screenshot and screen recorder");
        res.set("InternalName", "rustsnip");
        res.set("OriginalFilename", "rustsnip.exe");
        res.compile().expect("failed to embed Windows resources");
    }
}
