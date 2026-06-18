; bpsr-module-optimizer — Tauri NSIS インストーラフック。
;
; 目的: WinDivert を使う姉妹アプリ bpsr-checker（自前 NSIS）と同等の
; インストール/アンインストール処理を、stock Tauri NSIS 上で実現する。
;
; "WinDivert" サービス／ドライバは**マシン全体で1つの共有資源**。本フックは
; ドライバの所有者ではなく善良な利用者として振る舞う:
;   * サービスを **delete しない**（他の WinDivert 利用アプリと共存するため）。
;     残留した壊れたサービスはアプリ起動時に自己修復する（recover_stale_service）。
;   * 同梱 WinDivert64.sys の上書き(install)・削除(uninstall)の前に、ドライバを
;     停止してファイルロックを解放する。
;
; 順序の注意: Tauri は本フックの **直後** に CheckIfAppIsRunning でアプリを終了する
; （PRE*INSTALL フック → アプリ終了 → ファイル操作 の順）。そのため sc stop の前に
; 自前で taskkill してアプリのハンドルを解放しておかないと、アプリが WinDivert を
; 掴んだままで sc stop が拒否され、.sys ロックが解けない。checker の KillAppAndDriver
; と同じ「taskkill → sc stop」の順序を踏襲する。
; sc stop は他プロセスが使用中なら拒否されるため、共存していても相手を壊さない。

!macro StopAppAndDriver
  nsExec::ExecToLog 'taskkill /F /IM ${MAINBINARYNAME}.exe /T'
  Sleep 500
  nsExec::ExecToLog 'sc stop WinDivert'
  Sleep 1000
!macroend

; インストール（更新時に旧 .sys がロックされ得るので、上書きコピー前に解放）。
!macro NSIS_HOOK_PREINSTALL
  !insertmacro StopAppAndDriver
!macroend

; アンインストール（.sys 削除前にロックを解放）。
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro StopAppAndDriver
!macroend

; 削除フォールバック: 他アプリがこの .sys からロードしたドライバを掴んでいる等で
; sc stop が拒否されロックが残った場合に備え、WinDivert 同梱物を再起動時削除へ回す
; （Tauri 標準の Delete は /REBOOTOK を付けないため残骸が出るのを防ぐ）。
; 更新時($UpdateMode)は除外する: 更新は「旧アンインストール→新インストール」の順で走るため、
; ここで再起動時削除を予約すると、直後に再配置された新しい .sys を巻き込む恐れがある。
; （通常のアンインストールでは PREUNINSTALL で停止済みなので Delete は即時成功し、ここは
;  ファイル不在の no-op になる。ロックが残った真のアンインストール時のみ予約が効く。）
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Delete /REBOOTOK "$INSTDIR\WinDivert64.sys"
    Delete /REBOOTOK "$INSTDIR\WinDivert.dll"
    RMDir /REBOOTOK "$INSTDIR"
  ${EndIf}
!macroend
