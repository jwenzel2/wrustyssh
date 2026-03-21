fn main() {
    slint_build::compile("src/ui/window.slint").unwrap();
    let _ = embed_resource::compile("app.rc", embed_resource::NONE);
}
