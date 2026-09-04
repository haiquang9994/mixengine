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

; Past this, `ReadRegStr` may have handed back a truncated value — see `AddToPath`.
!define PATH_LIMIT 1000

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

; "Is $1 somewhere inside $0?" — leaves 1 in $2 when it is and 0 when it is not.
;
; **A macro and not a function, and both of these are.** The NSIS convention for a function is to
; take its arguments on the stack and hand the result back the same way, which is four `Exch`es
; whose ordering is easy to get subtly wrong and impossible to test from here. Nothing outside this
; file calls either of these, so there is no convention to keep: expanded inline, they are ordinary
; straight-line code over `$0`–`$5` and a reader can check them by reading them.
!macro StrFind
  StrLen $3 $1
  StrCpy $4 0
  StrCpy $2 0
  ${Do}
    StrCpy $5 $0 $3 $4
    ${If} $5 == ""
      ${ExitDo}
    ${EndIf}
    ${If} $5 == $1
      StrCpy $2 1
      ${ExitDo}
    ${EndIf}
    IntOp $4 $4 + 1
  ${Loop}
!macroend

; "$0 with the first occurrence of $1 removed" — leaves the result in $0.
!macro StrCut
  StrLen $3 $1
  StrCpy $4 0
  ${Do}
    StrCpy $5 $0 $3 $4
    ${If} $5 == ""
      ${ExitDo}
    ${EndIf}
    ${If} $5 == $1
      StrCpy $6 $0 $4
      IntOp $7 $4 + $3
      StrCpy $7 $0 "" $7
      StrCpy $0 "$6$7"
      ${ExitDo}
    ${EndIf}
    IntOp $4 $4 + 1
  ${Loop}
!macroend

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
  StrLen $8 $0

  ${If} $8 >= ${PATH_LIMIT}
    DetailPrint "This account's PATH is too long for the installer to edit safely."
    DetailPrint "Add $INSTDIR to it by hand, or run: mix path install"
    Return
  ${EndIf}

  StrCpy $1 "$INSTDIR"
  !insertmacro StrFind

  ${If} $2 == 1
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
;
; The separator is removed with the directory rather than after it, so a PATH that held only this
; entry does not end up as a lone `;` — and the second pass covers the case where the entry was
; first in the value and had no separator in front of it.
Function un.RemoveFromPath
  ReadRegStr $0 HKCU "Environment" "Path"
  StrLen $8 $0

  ${If} $8 >= ${PATH_LIMIT}
    DetailPrint "This account's PATH is too long to edit safely; $INSTDIR was left on it."
    Return
  ${EndIf}

  StrCpy $1 ";$INSTDIR"
  !insertmacro StrCut

  StrCpy $1 "$INSTDIR"
  !insertmacro StrCut

  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
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
