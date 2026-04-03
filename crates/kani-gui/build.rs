fn main() {
    #[cfg(target_os = "windows")]
    {
        winres::WindowsResource::new()
            .set_icon("assets/app.ico")
            .set("ProductName", "Kani KVM")
            .set("FileDescription", "Cross-platform software KVM")
            .compile()
            .expect("Failed to compile Windows resources");
    }
}
