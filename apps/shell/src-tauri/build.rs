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

    tauri_build::build()
}
