; MixEngine — a per-user installer.
;
; `RequestExecutionLevel user` is the whole point: nothing here asks for UAC, so an update needs no
; administrator. The one file that must live somewhere an ordinary account cannot rewrite,
; `mixengine-elevate.exe`, is *not* placed by this installer — MixEngine installs it itself, inside
; the elevation prompt first-run setup already costs. See ADR 0015 and the T85 design, D1.
;
; Driven by packaging/windows/build.sh, which defines VERSION, STAGE and OUTFILE.

Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "WinMessages.nsh"
!include "LogicLib.nsh"

!define NAME "MixEngine"
!define PUBLISHER "MixEngine"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\MixEngine"

Name "${NAME} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\MixEngine"
InstallDirRegKey HKCU "Software\MixEngine" "InstallDir"
ShowInstDetails show
ShowUninstDetails show

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetOutPath "$INSTDIR"
  File "${STAGE}\mix.exe"
  File "${STAGE}\mixengined.exe"
  File "${STAGE}\mixengine-elevate.exe"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\MixEngine" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\mix.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

  Call AddToPath
SectionEnd

; Append $INSTDIR to this user's PATH — and refuse rather than risk it.
;
; **The guard is not decoration.** NSIS's `ReadRegStr` silently truncates at `NSIS_MAX_STRLEN`, so
; writing back what it read can cut a long PATH in half. A PATH that was not extended is an
; inconvenience; a PATH that was truncated is somebody's afternoon. See the T85 design, D10.
;
; `<root>/bin` — the directory of runtime shims — is deliberately *not* written here. That one
; belongs to `path.install`, which writes it when somebody asks and takes it back off again; the two
; therefore own different segments of one value, which is what makes two authors safe.
Function AddToPath
  ReadRegStr $0 HKCU "Environment" "Path"
  StrLen $1 $0

  ${If} $1 >= 1000
    DetailPrint "This account's PATH is too long for the installer to edit safely."
    DetailPrint "Add $INSTDIR to it by hand, or run: mix path install"
    Return
  ${EndIf}

  Push $0
  Push "$INSTDIR"
  Call StrContains
  Pop $2

  ${If} $2 == "found"
    Return
  ${EndIf}

  ${If} $0 == ""
    WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
  ${Else}
    WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
  ${EndIf}

  ; So a shell started from Explorer afterwards picks it up without a logout.
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
FunctionEnd

; Take exactly our own segment back out, leaving the rest of the value as it was.
Function un.RemoveFromPath
  ReadRegStr $0 HKCU "Environment" "Path"
  StrLen $1 $0

  ${If} $1 >= 1000
    DetailPrint "This account's PATH is too long to edit safely; $INSTDIR was left on it."
    Return
  ${EndIf}

  Push $0
  Push ";$INSTDIR"
  Call un.StrCut
  Pop $0

  Push $0
  Push "$INSTDIR"
  Call un.StrCut
  Pop $0

  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
FunctionEnd

; "Does $R0 contain $R1?" — push haystack, push needle, pop "found" or "".
!macro StrContainsBody
  Exch $R1
  Exch
  Exch $R0
  Push $R2
  Push $R3
  Push $R4
  StrLen $R2 $R1
  StrCpy $R3 0
  loop:
    StrCpy $R4 $R0 $R2 $R3
    StrCmp $R4 "" notfound
    StrCmp $R4 $R1 found
    IntOp $R3 $R3 + 1
    Goto loop
  found:
    StrCpy $R0 "found"
    Goto done
  notfound:
    StrCpy $R0 ""
  done:
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Exch $R0
!macroend

; "$R0 with the first occurrence of $R1 removed" — push haystack, push needle, pop the result.
!macro StrCutBody
  Exch $R1
  Exch
  Exch $R0
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  StrLen $R2 $R1
  StrCpy $R3 0
  loop:
    StrCpy $R4 $R0 $R2 $R3
    StrCmp $R4 "" done
    StrCmp $R4 $R1 cut
    IntOp $R3 $R3 + 1
    Goto loop
  cut:
    StrCpy $R4 $R0 $R3
    IntOp $R5 $R3 + $R2
    StrCpy $R5 $R0 "" $R5
    StrCpy $R0 "$R4$R5"
  done:
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Exch $R0
!macroend

Function StrContains
  !insertmacro StrContainsBody
FunctionEnd

Function un.StrCut
  !insertmacro StrCutBody
FunctionEnd

Section "Uninstall"
  ; **Only the files this installer wrote.** What MixEngine did to the *machine* — the hosts block,
  ; the resolver wiring, the CA in every store, the port grant, and the helper it installed — is
  ; `mix uninstall`'s, which is roadmap task T87 and does not exist yet. Saying so in the log beats
  ; pretending this removed it.
  DetailPrint "Removing the files this installer wrote."
  DetailPrint "What MixEngine changed on this machine is removed by `mix uninstall` (not yet built)."

  Call un.RemoveFromPath

  Delete "$INSTDIR\mix.exe"
  Delete "$INSTDIR\mixengined.exe"
  Delete "$INSTDIR\mixengine-elevate.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\MixEngine"
SectionEnd
