fn main() {
    unsafe{hello_world()}
}

#[link(wasm_import_module = "libhello.wasm")]
extern {
    fn hello_world();
}