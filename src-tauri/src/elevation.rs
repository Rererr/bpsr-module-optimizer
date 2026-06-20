//! Windows 実行時の自己昇格。
//!
//! ライブキャプチャ（WinDivert）は管理者権限を要求する。マニフェストの
//! `requireAdministrator` ではなく、起動時に未昇格を検知したら ShellExecute の
//! `runas` 動詞で自分自身を昇格して再起動し、元プロセスは終了する方式を採る。
//! こうすることでマニフェストは `asInvoker` 固定にでき、二重埋め込みによるリンク
//! 失敗を避けつつ、dev では環境変数で昇格をスキップできる。
//!
//! 非 Windows ビルドでは何もしない。

/// 未昇格で起動された場合に自分自身を昇格して再起動し、元プロセスを終了する。
///
/// - 既に昇格済み: そのまま続行する。
/// - 未昇格: `runas` で再起動を試み、本プロセスは終了する（成功時は昇格済み
///   プロセスが処理を引き継ぐ。UAC 拒否・キャンセル時もそのまま終了する）。
/// - `BPSR_SKIP_ELEVATION=1`: 昇格をスキップして続行する（dev 用）。
#[cfg(target_os = "windows")]
pub fn ensure_elevated() {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if std::env::var("BPSR_SKIP_ELEVATION").is_ok_and(|v| v == "1") {
        log::info!("[elevation] BPSR_SKIP_ELEVATION=1 のため昇格をスキップ");
        return;
    }

    if is_elevated() {
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            // パスが取れないと昇格できない。続行はするが管理者必須機能は失敗する。
            log::error!("[elevation] 実行ファイルパス取得失敗のため昇格をスキップ: {e}");
            return;
        }
    };

    // 起動引数を引き継ぐ（GUI 起動時は通常空）。スペース等を含む値のため各引数を
    // ダブルクォートで囲い、内部のダブルクォートはエスケープする。
    let params: String = std::env::args()
        .skip(1)
        .map(|a| format!("\"{}\"", a.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" ");

    let exe_w = to_wide(exe.as_os_str());
    let verb_w = to_wide_str("runas");
    let params_w = to_wide_str(&params);

    // ShellExecuteW は成功時 32 超の値を返す。32 以下は失敗（ユーザー拒否時の
    // ERROR_CANCELLED を含む）。
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            exe_w.as_ptr(),
            if params.is_empty() {
                std::ptr::null()
            } else {
                params_w.as_ptr()
            },
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    if (ret as isize) <= 32 {
        log::warn!(
            "[elevation] 管理者への昇格が拒否またはキャンセルされました（コード {}）。終了します。",
            ret as isize
        );
    }
    // 成功・失敗いずれの場合も本プロセスは終了する。
    std::process::exit(0);
}

/// 現在のプロセスが昇格済み（管理者トークン）かどうかを返す。
#[cfg(target_os = "windows")]
fn is_elevated() -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION {
            TokenIsElevated: 0,
        };
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(target_os = "windows")]
fn to_wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn to_wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 非 Windows では何もしない。
#[cfg(not(target_os = "windows"))]
pub fn ensure_elevated() {}
