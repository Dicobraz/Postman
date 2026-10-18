// Postman: шейдеры не компилируются при сборке (fxc.exe нет при кросс-сборке из Linux),
// см. shader_resources в directx_renderer.rs.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
