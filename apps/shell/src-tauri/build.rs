use std::env;

fn main() {
    // 菜单包/业务包验签公钥：生产构建必须由 CI 注入，演示公钥不得进入发布产物。
    // 调试构建允许缺省（lib.rs 会回退演示公钥），保证 `cargo run` 开箱可用。
    println!("cargo:rerun-if-env-changed=TK_MENU_PUBKEY_HEX");
    match env::var("TK_MENU_PUBKEY_HEX") {
        Ok(key) if !key.trim().is_empty() => {
            println!("cargo:rustc-env=TK_MENU_PUBKEY_HEX={}", key.trim());
        }
        _ if env::var("PROFILE").as_deref() == Ok("release") => {
            panic!(
                "release 构建必须设置 TK_MENU_PUBKEY_HEX（菜单包验签公钥）；\
                 演示公钥不得进入发布产物"
            );
        }
        _ => {}
    }

    // 端点配置验签公钥：生产构建同样必须由 CI 注入，运行期环境变量不得替换信任根。
    // debug 构建允许缺省（可用 TK_CONFIG_PUBKEY 临时覆盖），保证 `cargo run` 开箱可用。
    println!("cargo:rerun-if-env-changed=TK_CONFIG_PUBKEY_HEX");
    match env::var("TK_CONFIG_PUBKEY_HEX") {
        Ok(key) if !key.trim().is_empty() => {
            println!("cargo:rustc-env=TK_CONFIG_PUBKEY_HEX={}", key.trim());
        }
        _ if env::var("PROFILE").as_deref() == Ok("release") => {
            panic!(
                "release 构建必须设置 TK_CONFIG_PUBKEY_HEX（端点配置验签公钥）；\
                 不允许运行期替换信任根"
            );
        }
        _ => {}
    }

    tauri_build::build()
}
