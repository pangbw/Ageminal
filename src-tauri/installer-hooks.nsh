; Ageminal NSIS hooks (#90)
;
; 卸载前：
; 1. 通过 daemon 协议优雅结束所有会话与 daemon（不直接 taskkill）；
; 2. 仅当 agent 注入物仍带 Ageminal 标记时才清理；
; 3. 保留 %LOCALAPPDATA%\Ageminal\ 用户数据。
!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping Ageminal daemon and sessions..."
  nsExec::ExecToLog '"$INSTDIR\ageminal-daemon.exe" --shutdown'
  Pop $0

  DetailPrint "Removing Ageminal agent integrations (marked files only)..."
  nsExec::ExecToLog '"$INSTDIR\ageminal-desktop.exe" --uninstall-cleanup'
  Pop $0
!macroend
