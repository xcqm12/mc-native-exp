//! Windows 资源内嵌：exe 图标（brand/icon.ico，PIL 从 logo 生成）与版本信息。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("brand/icon.ico");
        res.set("ProductName", "mc-native-exp");
        res.set("FileDescription", "MC 原生启动实验（实验项目，可能不稳定）");
        if let Err(e) = res.compile() {
            println!("cargo:warning=winresource 编译失败：{e}（图标跳过，不影响功能）");
        }
    }
    println!("cargo:rerun-if-changed=brand/icon.ico");
}
