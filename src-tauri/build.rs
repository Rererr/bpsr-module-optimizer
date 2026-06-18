fn main() {
    let mut attributes = tauri_build::Attributes::new();

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rerun-if-env-changed=BPSR_SKIP_MANIFEST");

        // Tauri 本体もマニフェストを埋め込むため、embed-manifest 等で二重に埋め込むと
        // リンク時に CVT1100 (duplicate resource: MANIFEST) で失敗する。
        // Tauri の WindowsAttributes 経由で単一マニフェストとして管理者権限を要求する。
        // 開発時は BPSR_SKIP_MANIFEST=1 で asInvoker に切替え、昇格ダイアログを回避できる。
        let level = if std::env::var("BPSR_SKIP_MANIFEST").is_ok_and(|v| v == "1") {
            "asInvoker"
        } else {
            "requireAdministrator"
        };

        // Tauri 既定マニフェストを踏襲しつつ実行レベルのみ変更（DPI/Common Controls/長パス対応を維持）。
        let manifest = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    <dependency>
        <dependentAssembly>
            <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
        </dependentAssembly>
    </dependency>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}" />
        </application>
    </compatibility>
    <application xmlns="urn:schemas-microsoft-com:asm.v3">
        <windowsSettings>
            <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
            <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
        </windowsSettings>
    </application>
    <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
        <security>
            <requestedPrivileges>
                <requestedExecutionLevel level="{level}" uiAccess="false" />
            </requestedPrivileges>
        </security>
    </trustInfo>
</assembly>"#
        );

        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(manifest));
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
