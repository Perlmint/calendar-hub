fn main() {
    if cfg!(feature = "embed_web") {
        std::process::Command::new("sh")
            .args(["-C", "npx webpack"])
            .env("PRODUCTION", "1")
            .output()
            .unwrap();
    }
}
