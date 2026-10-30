; Extra installer steps for All Download Manager.

!macro NSIS_HOOK_POSTUNINSTALL
  ; Forget the browser extension's connection to the app.
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.alldownloadmanager.host"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.alldownloadmanager.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.alldownloadmanager.host"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\com.alldownloadmanager.host"
!macroend
