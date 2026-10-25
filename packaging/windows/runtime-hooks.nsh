; Runtime updates keep legacy bundled weights as a fallback during /UPDATE.
; The final app uninstall still removes those app-owned resources.
; User recordings and the managed model store live outside $INSTDIR.
!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$INSTDIR\native\models"
  RMDir "$INSTDIR\native"
!macroend
