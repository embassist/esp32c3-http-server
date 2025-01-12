# TODO:
1. Remove `env` WiFi parts from `.cargo/config.toml` moving to cfg.toml using `toml-config` crate.
2. Move to `project` file structure from current `lib`, making `esp-idf-svc` a dependency.

# Setup:
1. Rust nightly (see cxllmerichie/cloud)
2. ```shell
   cargo espflash flash --target riscv32imc-esp-espidf --example http_server --monitor
   ```