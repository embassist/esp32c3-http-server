### ToDo:
1. Make use of https://github.com/filipton/esp-hal-mdns.
2. Move project structure into non-lib setup, with `esp-hal` as dependency, using https://github.com/esp-rs/esp-generate
accordingly to https://docs.esp-rs.org/book/writing-your-own-application/generate-project/index.html.

### Notes:
1. Since of: https://github.com/embassy-rs/static-cell/issues/16, the note
"// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html"
would not work, thus nightly usage makes no sense (despite possible).
2. Code taken from about "https://github.com/esp-rs/esp-hal/commit/571760884bf90240fdace3afdfb7f527194681aa".

### Setup:
1. ```shell
   rustup toolchain install stable --component rust-src
   ```
2. ```shell
   rustup default stable
   ```
3. ```shell
   cargo xtask run-example esp-hal esp32c3 embassy_hello_world
   ```