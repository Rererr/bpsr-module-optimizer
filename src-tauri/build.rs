fn main() {
    let mut attributes = tauri_build::Attributes::new();

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // 管理者権限は実行時の自己昇格（src/elevation.rs）で取得するため、
        // マニフェストの実行レベルは asInvoker 固定とする。
        // requireAdministrator を埋め込むと dev でも常に UAC が出て自己昇格の
        // スキップ制御（BPSR_SKIP_ELEVATION）が効かず、マニフェスト二重埋め込みに
        // よるリンク失敗（CVT1100）の温床にもなるため避ける。
        // Tauri 既定マニフェストを踏襲し、DPI/Common Controls/長パス対応を維持する。
        let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    <dependency>
        <dependentAssembly>
            <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
        </dependentAssembly>
    </dependency>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
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
                <requestedExecutionLevel level="asInvoker" uiAccess="false" />
            </requestedPrivileges>
        </security>
    </trustInfo>
</assembly>"#;

        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(manifest));
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
